//! psh-polkit — polkit authentication agent for the psh desktop environment.
//!
//! Registers as a polkit agent on the D-Bus system bus and shows a GTK4
//! layer-shell password dialog when privileged actions are requested. Supports
//! concurrent per-session authentication, password verification via
//! `polkit-agent-helper-1`, NSS username resolution, and password zeroization.

#![allow(dead_code, unused_imports)]

mod agent;
mod authority;

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use secrecy::SecretString;
use zeroize::Zeroize;

use agent::{AgentToGtk, AuthRequest, AuthResponse};

// ---------------------------------------------------------------------------
// Active dialog state
// ---------------------------------------------------------------------------

/// Holds references to the active dialog widgets so we can update them on
/// retry or close them on cancel/success.
struct DialogState {
    window: gtk4::ApplicationWindow,
    entry: gtk4::PasswordEntry,
    error_label: gtk4::Label,
    spinner: gtk4::Spinner,
    auth_btn: gtk4::Button,
    cancel_btn: gtk4::Button,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    psh_core::logging::init("psh_polkit");
    tracing::info!("starting psh-polkit");

    let cfg = psh_core::config::load().expect("failed to load config");

    let app = gtk4::Application::builder()
        .application_id("com.psh.polkit")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        let (gtk_tx, gtk_rx) = async_channel::bounded::<AgentToGtk>(4);
        let (resp_tx, resp_rx) = async_channel::bounded::<AuthResponse>(4);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Wrap shutdown sender so we can take it on app shutdown
        let shutdown_tx = Rc::new(RefCell::new(Some(shutdown_tx)));

        // Start polkit agent on background thread
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = agent::run(gtk_tx, resp_rx, shutdown_rx).await {
                    tracing::error!("polkit agent error: {e}");
                }
            });
        });

        // Track the active dialog
        let dialog: Rc<RefCell<Option<DialogState>>> = Rc::new(RefCell::new(None));

        // Signal graceful shutdown when the app closes
        let shutdown_tx_clone = shutdown_tx.clone();
        app.connect_shutdown(move |_| {
            if let Some(tx) = shutdown_tx_clone.borrow_mut().take() {
                let _ = tx.send(());
            }
        });

        // Dispatch messages from the agent thread to the GTK UI
        let app_clone = app.clone();
        let dialog_clone = dialog.clone();
        let resp_tx_clone = resp_tx.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = gtk_rx.recv().await {
                match msg {
                    AgentToGtk::ShowAuth(req) => {
                        show_auth_dialog(&app_clone, &req, &resp_tx_clone, &dialog_clone);
                    }
                    AgentToGtk::AuthFailed { message, .. } => {
                        update_dialog_error(&dialog_clone, &message);
                    }
                    AgentToGtk::AuthCancelled { .. } | AgentToGtk::AuthSucceeded { .. } => {
                        close_dialog(&dialog_clone);
                    }
                }
            }
        });
    });

    app.run_with_args::<String>(&[]);
}

// ---------------------------------------------------------------------------
// Dialog management
// ---------------------------------------------------------------------------

/// Create and show the authentication dialog.
fn show_auth_dialog(
    app: &gtk4::Application,
    req: &AuthRequest,
    resp_tx: &async_channel::Sender<AuthResponse>,
    dialog: &Rc<RefCell<Option<DialogState>>>,
) {
    // Close any existing dialog first
    close_dialog(dialog);

    let window = gtk4::ApplicationWindow::builder()
        .application(app)
        .default_width(400)
        .default_height(220)
        .decorated(false)
        .build();

    window.init_layer_shell();
    window.set_layer(gtk4_layer_shell::Layer::Overlay);
    window.set_keyboard_mode(gtk4_layer_shell::KeyboardMode::Exclusive);

    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    container.add_css_class("psh-polkit-dialog");
    container.set_margin_top(24);
    container.set_margin_bottom(24);
    container.set_margin_start(24);
    container.set_margin_end(24);

    // Title
    let title = gtk4::Label::new(Some("Authentication Required"));
    title.add_css_class("psh-polkit-title");
    container.append(&title);

    // Message
    let message = gtk4::Label::new(Some(&req.message));
    message.set_wrap(true);
    container.append(&message);

    // Username label
    let user_label = gtk4::Label::new(Some(&format!("Password for {}:", req.username)));
    user_label.set_halign(gtk4::Align::Start);
    container.append(&user_label);

    // Password entry
    let entry = gtk4::PasswordEntry::new();
    entry.set_show_peek_icon(true);
    container.append(&entry);

    // Error label (hidden initially)
    let error_label = gtk4::Label::new(None);
    error_label.add_css_class("psh-polkit-error");
    error_label.set_visible(false);
    error_label.set_wrap(true);
    container.append(&error_label);

    // Spinner (hidden initially, shown while authenticating)
    let spinner = gtk4::Spinner::new();
    spinner.set_visible(false);
    container.append(&spinner);

    // Button row
    let button_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    button_box.set_halign(gtk4::Align::End);

    let cancel_btn = gtk4::Button::with_label("Cancel");
    let auth_btn = gtk4::Button::with_label("Authenticate");
    auth_btn.add_css_class("suggested-action");

    button_box.append(&cancel_btn);
    button_box.append(&auth_btn);
    container.append(&button_box);

    window.set_child(Some(&container));

    // --- Cancel button handler ---
    let window_cancel = window.clone();
    let resp_tx_cancel = resp_tx.clone();
    let cookie_cancel = req.cookie.clone();
    cancel_btn.connect_clicked(move |_| {
        let _ = resp_tx_cancel.send_blocking(AuthResponse {
            cookie: cookie_cancel.clone(),
            password: None,
        });
        window_cancel.close();
    });

    // --- Authenticate button handler ---
    let submit = {
        let entry_submit = entry.clone();
        let resp_tx_auth = resp_tx.clone();
        let cookie_auth = req.cookie.clone();
        let error_label_auth = error_label.clone();
        let spinner_auth = spinner.clone();
        let auth_btn_auth = auth_btn.clone();
        move || {
            let mut text = entry_submit.text().to_string();
            if text.is_empty() {
                return;
            }
            let password = SecretString::from(text.as_str());
            text.zeroize();
            // Clear GTK's internal buffer as best-effort
            entry_submit.set_text("");

            // Hide error, disable inputs, show spinner
            error_label_auth.set_visible(false);
            entry_submit.set_sensitive(false);
            auth_btn_auth.set_sensitive(false);
            spinner_auth.set_visible(true);
            spinner_auth.start();

            let _ = resp_tx_auth.send_blocking(AuthResponse {
                cookie: cookie_auth.clone(),
                password: Some(password),
            });
        }
    };

    let submit_click = submit.clone();
    auth_btn.connect_clicked(move |_| {
        submit_click();
    });

    // Enter key submits
    let submit_enter = submit;
    entry.connect_activate(move |_| {
        submit_enter();
    });

    // Escape key cancels the dialog
    let key_controller = gtk4::EventControllerKey::new();
    let window_esc = window.clone();
    let resp_tx_esc = resp_tx.clone();
    let cookie_esc = req.cookie.clone();
    key_controller.connect_key_pressed(move |_, key, _, _| {
        if key == gtk4::gdk::Key::Escape {
            let _ = resp_tx_esc.send_blocking(AuthResponse {
                cookie: cookie_esc.clone(),
                password: None,
            });
            window_esc.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key_controller);

    // Store dialog state
    *dialog.borrow_mut() = Some(DialogState {
        window: window.clone(),
        entry,
        error_label,
        spinner,
        auth_btn,
        cancel_btn,
    });

    window.present();

    // Auto-cancel after 120 seconds to prevent indefinite keyboard grab
    let resp_tx_timeout = resp_tx.clone();
    let cookie_timeout = req.cookie.clone();
    let dialog_timeout = dialog.clone();
    glib::timeout_add_local_once(std::time::Duration::from_secs(120), move || {
        let borrow = dialog_timeout.borrow();
        if let Some(state) = borrow.as_ref()
            && state.window.is_visible()
        {
            let _ = resp_tx_timeout.send_blocking(AuthResponse {
                cookie: cookie_timeout.clone(),
                password: None,
            });
            state.window.close();
        }
    });
}

/// Update the active dialog to show an authentication error and allow retry.
fn update_dialog_error(dialog: &Rc<RefCell<Option<DialogState>>>, message: &str) {
    let borrow = dialog.borrow();
    let Some(state) = borrow.as_ref() else {
        return;
    };

    state.error_label.set_text(message);
    state.error_label.set_visible(true);
    state.spinner.stop();
    state.spinner.set_visible(false);
    state.entry.set_sensitive(true);
    state.auth_btn.set_sensitive(true);

    // Clear password and re-focus entry
    state.entry.set_text("");
    state.entry.grab_focus();
}

/// Close and drop the active dialog.
fn close_dialog(dialog: &Rc<RefCell<Option<DialogState>>>) {
    if let Some(state) = dialog.borrow_mut().take() {
        state.window.close();
    }
}
