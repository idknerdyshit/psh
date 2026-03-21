#![allow(dead_code, unused_imports)]

mod modules;

use gtk4::prelude::*;
use gtk4_layer_shell::LayerShell;
use psh_core::config::{self, BarPosition};

fn main() {
    psh_core::logging::init("psh_bar");
    tracing::info!("starting psh-bar");

    let cfg = config::load().expect("failed to load config");
    let bar_cfg = cfg.bar.clone();

    let app = gtk4::Application::builder()
        .application_id("com.psh.bar")
        .build();

    app.connect_activate(move |app| {
        psh_core::theme::apply_theme(&cfg.theme.name);

        // Start IPC hub on background thread
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Err(e) = run_ipc_hub().await {
                    tracing::error!("IPC hub error: {e}");
                }
            });
        });

        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .build();

        window.init_layer_shell();
        window.set_layer(gtk4_layer_shell::Layer::Top);
        window.set_anchor(gtk4_layer_shell::Edge::Left, true);
        window.set_anchor(gtk4_layer_shell::Edge::Right, true);

        match bar_cfg.position {
            BarPosition::Top => window.set_anchor(gtk4_layer_shell::Edge::Top, true),
            BarPosition::Bottom => window.set_anchor(gtk4_layer_shell::Edge::Bottom, true),
        }

        let height = bar_cfg.height.unwrap_or(32);
        window.set_default_height(height as i32);
        window.set_namespace(Some("psh-bar"));

        let bar = gtk4::CenterBox::new();
        bar.add_css_class("psh-bar");

        // Left modules
        let left = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        left.set_margin_start(8);
        left.add_css_class("psh-bar-left");
        left.append(&modules::workspaces::widget());
        bar.set_start_widget(Some(&left));

        // Center modules
        let center = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        center.add_css_class("psh-bar-center");
        center.append(&modules::clock::widget());
        bar.set_center_widget(Some(&center));

        // Right modules
        let right = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        right.set_margin_end(8);
        right.add_css_class("psh-bar-right");
        right.append(&modules::battery::widget());
        bar.set_end_widget(Some(&right));

        window.set_child(Some(&bar));
        window.present();
    });

    app.run_with_args::<String>(&[]);
}

async fn run_ipc_hub() -> psh_core::Result<()> {
    let listener = psh_core::ipc::bind().await?;
    tracing::info!("IPC hub listening");

    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| psh_core::PshError::Ipc(e.to_string()))?;

        tokio::spawn(async move {
            while let Ok(msg) = psh_core::ipc::recv(&mut stream).await {
                tracing::debug!("hub received: {msg:?}");
                if let psh_core::ipc::Message::Ping = msg {
                    let _ =
                        psh_core::ipc::send(&mut stream, &psh_core::ipc::Message::Pong).await;
                }
            }
        });
    }
}
