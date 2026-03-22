//! psh — CLI control tool for the psh desktop environment.
//!
//! Sends IPC commands to psh-bar (the central hub) over a Unix socket.

use clap::{Parser, Subcommand};
use psh_core::ipc::Message;

#[derive(Parser)]
#[command(name = "psh", about = "Control the psh desktop environment")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lock the screen.
    Lock,
    /// Toggle the application launcher.
    Launcher,
    /// Show the clipboard history picker.
    Clipboard,
    /// Broadcast a config-reload signal to all components.
    Reload,
    /// Check if the IPC hub is running.
    Ping,
    /// Control the wallpaper.
    Wall {
        #[command(subcommand)]
        action: WallAction,
    },
    /// Manage GTK/Qt themes to match the psh palette.
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
    },
}

#[derive(Subcommand)]
enum WallAction {
    /// Set the wallpaper to the given image path.
    Set {
        /// Path to the wallpaper image.
        path: String,
    },
}

#[derive(Subcommand)]
enum ThemeAction {
    /// Generate and install GTK3/GTK4 CSS overrides and Qt5ct/Qt6ct color
    /// schemes that match the current psh palette.
    Apply,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    psh_core::logging::init("psh");

    let cli = Cli::parse();

    // Handle commands that don't go through IPC.
    if let Command::Theme { action } = &cli.command {
        handle_theme(action);
        return;
    }

    let msg = match cli.command {
        Command::Lock => Message::LockScreen,
        Command::Launcher => Message::ToggleLauncher,
        Command::Clipboard => Message::ShowClipboardHistory,
        Command::Reload => Message::ConfigReloaded,
        Command::Ping => Message::Ping,
        Command::Wall { action } => match action {
            WallAction::Set { path } => Message::SetWallpaper { path },
        },
        Command::Theme { .. } => unreachable!(),
    };

    let wait_for_response = matches!(msg, Message::Ping);

    let mut stream = match psh_core::ipc::connect().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to connect to psh IPC hub: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = psh_core::ipc::send(&mut stream, &msg).await {
        eprintln!("failed to send message: {e}");
        std::process::exit(1);
    }

    if wait_for_response {
        match psh_core::ipc::recv(&mut stream).await {
            Ok(Message::Pong) => println!("pong"),
            Ok(other) => println!("unexpected response: {other:?}"),
            Err(e) => {
                eprintln!("failed to read response: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Handle `psh theme` subcommands.
fn handle_theme(action: &ThemeAction) {
    match action {
        ThemeAction::Apply => {
            let config = match psh_core::config::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("failed to load config: {e}");
                    std::process::exit(1);
                }
            };

            let palette = psh_core::palette::Palette::load(&config.theme.name);
            if palette.colors.is_empty() {
                eprintln!("no colors found in theme '{}'", config.theme.name);
                std::process::exit(1);
            }

            match palette.apply() {
                Ok(paths) => {
                    println!("applied psh palette to system themes:");
                    for path in &paths {
                        println!("  {}", path.display());
                    }
                    println!();
                    println!("note: GTK apps will pick up changes on next launch.");
                    println!("      Qt apps require selecting the 'psh' color scheme in qt5ct/qt6ct.");
                }
                Err(e) => {
                    eprintln!("failed to apply theme: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}
