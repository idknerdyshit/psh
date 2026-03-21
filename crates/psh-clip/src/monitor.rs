//! Clipboard monitor using the `zwlr-data-control-v1` Wayland protocol.
//!
//! Opens an independent Wayland connection (separate from GTK's) on a background
//! thread, monitors clipboard changes, and can set the clipboard for paste-on-select.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{debug, error, info, warn};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1, zwlr_data_control_manager_v1, zwlr_data_control_offer_v1,
    zwlr_data_control_source_v1,
};

use crate::history::{ClipEntry, ClipHistory};
use crate::persist;
use psh_core::config::ClipConfig;

/// MIME types we look for, in preference order.
const TEXT_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

/// Image MIME types we look for, in preference order.
const IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/bmp"];

/// Internal state for a pending clipboard offer.
///
/// Accumulates MIME types as the compositor advertises them, before the
/// selection event triggers a read.
#[derive(Debug, Default)]
struct OfferData {
    /// MIME types offered by this clipboard content, in the order advertised.
    mime_types: Vec<String>,
}

/// Data attached to an active data-control source we created for paste-on-select.
struct SourceData {
    /// The clipboard entry to serve when a consumer requests data.
    entry: ClipEntry,
}

/// Main state for the clipboard monitor thread.
///
/// Holds the Wayland protocol objects, pending offers, and channels for
/// communicating clipboard entries back to the GTK thread.
struct MonitorState {
    /// Shared clipboard history for duplicate detection.
    history: ClipHistory,
    /// Channel to send new clipboard entries to the GTK thread.
    clip_tx: async_channel::Sender<ClipEntry>,
    /// Clipboard configuration (max image size, image support toggle, etc.).
    cfg: ClipConfig,
    /// The Wayland seat (input group) we're monitoring.
    seat: Option<wl_seat::WlSeat>,
    /// The `zwlr_data_control_manager_v1` global for creating devices and sources.
    manager: Option<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1>,
    /// The data-control device bound to our seat, delivering selection events.
    device: Option<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1>,
    /// Pending offers keyed by their Wayland proxy, accumulating MIME types.
    offers: HashMap<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, OfferData>,
    /// When true, the next selection event is from our own `set_clipboard` and should be ignored.
    skip_next_selection: bool,
    /// Active data-control source kept alive until the compositor cancels it.
    _active_source: Option<zwlr_data_control_source_v1::ZwlrDataControlSourceV1>,
}

/// Runs the clipboard monitor loop. Call from a dedicated background thread.
///
/// Connects to Wayland, binds the data-control protocol, and monitors clipboard
/// changes. Clipboard entries are pushed to `clip_tx` for the GTK thread to display.
/// Entries to set as the clipboard are received from `set_rx`.
pub fn run_monitor(
    history: ClipHistory,
    clip_tx: async_channel::Sender<ClipEntry>,
    set_rx: mpsc::Receiver<ClipEntry>,
    cfg: ClipConfig,
) {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            error!("clipboard monitor: failed to connect to Wayland: {e}");
            return;
        }
    };

    let display = conn.display();
    let mut event_queue: EventQueue<MonitorState> = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = MonitorState {
        history,
        clip_tx,
        cfg,
        seat: None,
        manager: None,
        device: None,
        offers: HashMap::new(),
        skip_next_selection: false,
        _active_source: None,
    };

    // Subscribe to registry events to discover globals
    display.get_registry(&qh, ());

    // Initial roundtrip to receive globals
    event_queue.roundtrip(&mut state).unwrap_or_else(|e| {
        error!("clipboard monitor: initial roundtrip failed: {e}");
        0
    });

    // Bind the data device if we have both manager and seat
    if let (Some(manager), Some(seat)) = (&state.manager, &state.seat) {
        let device = manager.get_data_device(seat, &qh, ());
        state.device = Some(device);
        info!("clipboard monitor: data control device bound");
    } else {
        if state.manager.is_none() {
            error!("clipboard monitor: compositor does not support zwlr_data_control_manager_v1");
        }
        if state.seat.is_none() {
            error!("clipboard monitor: no wl_seat found");
        }
        return;
    }

    // Second roundtrip to get initial selection
    let _ = event_queue.roundtrip(&mut state);

    info!("clipboard monitor: entering event loop");

    loop {
        // Flush outgoing requests
        if let Err(e) = conn.flush() {
            error!("clipboard monitor: flush error: {e}");
            break;
        }

        // Prepare to read — hold the guard across poll so no other thread reads
        let guard = match conn.prepare_read() {
            Some(g) => g,
            None => {
                // Events already queued, just dispatch them
                if let Err(e) = event_queue.dispatch_pending(&mut state) {
                    error!("clipboard monitor: dispatch error: {e}");
                    break;
                }
                // Check for paste-on-select requests
                while let Ok(entry) = set_rx.try_recv() {
                    set_clipboard(&mut state, &qh, entry);
                }
                continue;
            }
        };

        // Poll the Wayland fd with a 100ms timeout
        let wayland_fd = guard.connection_fd();
        let timeout = rustix::time::Timespec {
            tv_sec: 0,
            tv_nsec: 100_000_000, // 100ms
        };
        let poll_result = rustix::event::poll(
            &mut [rustix::event::PollFd::new(&wayland_fd, rustix::event::PollFlags::IN)],
            Some(&timeout),
        );

        match poll_result {
            Ok(n) if n > 0 => {
                if let Err(e) = guard.read() {
                    error!("clipboard monitor: read error: {e}");
                    break;
                }
            }
            Ok(_) => {
                // Timeout or no data — drop the guard to cancel the read
                drop(guard);
            }
            Err(e) => {
                error!("clipboard monitor: poll error: {e}");
                drop(guard);
                break;
            }
        }

        // Dispatch any pending events
        if let Err(e) = event_queue.dispatch_pending(&mut state) {
            error!("clipboard monitor: dispatch error: {e}");
            break;
        }

        // Check for paste-on-select requests
        while let Ok(entry) = set_rx.try_recv() {
            set_clipboard(&mut state, &qh, entry);
        }
    }
}

/// Sets the clipboard to the given entry via data-control source.
fn set_clipboard(state: &mut MonitorState, qh: &QueueHandle<MonitorState>, entry: ClipEntry) {
    let Some(manager) = &state.manager else { return };
    let Some(device) = &state.device else { return };

    let source = manager.create_data_source(qh, SourceData {
        entry: entry.clone(),
    });

    match &entry {
        ClipEntry::Text { .. } => {
            source.offer("text/plain;charset=utf-8".to_string());
            source.offer("text/plain".to_string());
            source.offer("UTF8_STRING".to_string());
        }
        ClipEntry::Image { mime, .. } => {
            source.offer(mime.clone());
        }
    }

    device.set_selection(Some(&source));
    state.skip_next_selection = true;
    state._active_source = Some(source);
    debug!("clipboard set via data-control source");
}

/// Read data from a clipboard offer for the best matching MIME type.
///
/// `conn` is used to flush the receive request before polling for data.
fn read_offer(
    offer: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    offer_data: &OfferData,
    cfg: &ClipConfig,
    conn: &Connection,
) -> Option<ClipEntry> {
    // Try text MIME types first
    for mime in TEXT_MIMES {
        if !offer_data.mime_types.iter().any(|m| m == mime) {
            continue;
        }
        if let Some(data) = receive_offer_data(offer, mime, conn) {
            let text = String::from_utf8_lossy(&data).into_owned();
            if !text.is_empty() {
                return Some(ClipEntry::Text { content: text });
            }
        }
    }

    // Try image MIME types if enabled
    if cfg.image_support {
        for mime in IMAGE_MIMES {
            if !offer_data.mime_types.iter().any(|m| m == mime) {
                continue;
            }
            let Some(data) = receive_offer_data(offer, mime, conn) else {
                continue;
            };
            if data.len() > cfg.max_image_bytes {
                warn!(
                    "clipboard image too large ({} bytes > {} max), skipping",
                    data.len(),
                    cfg.max_image_bytes
                );
                return None;
            }
            let ext = match *mime {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/bmp" => "bmp",
                _ => "bin",
            };
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let Some(cache_dir) = persist::image_cache_dir() else {
                warn!("cannot determine image cache directory, skipping image");
                return None;
            };
            let _ = std::fs::create_dir_all(&cache_dir);
            let path = cache_dir.join(format!("clip_{ts}.{ext}"));
            if std::fs::write(&path, &data).is_ok() {
                return Some(ClipEntry::Image {
                    path,
                    mime: mime.to_string(),
                });
            }
        }
    }

    None
}

/// Receive data from an offer via a pipe. Returns None on failure.
///
/// Flushes `conn` after issuing the receive request so the compositor gets it
/// before we start polling the read end.
fn receive_offer_data(
    offer: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
    mime: &str,
    conn: &Connection,
) -> Option<Vec<u8>> {
    let (read_fd, write_fd) = pipe()?;
    offer.receive(mime.to_string(), write_fd.as_fd());

    // Flush so the compositor receives our request before we drop the write end
    let _ = conn.flush();

    // Drop write end so reads will see EOF once the compositor is done writing
    drop(write_fd);

    // Poll the read fd with a 2-second timeout
    let timeout = rustix::time::Timespec {
        tv_sec: 2,
        tv_nsec: 0,
    };
    let poll_result = rustix::event::poll(
        &mut [rustix::event::PollFd::new(&read_fd, rustix::event::PollFlags::IN)],
        Some(&timeout),
    );
    match poll_result {
        Ok(n) if n > 0 => {}
        _ => {
            warn!("clipboard offer read timed out for MIME {mime}");
            return None;
        }
    }

    let mut buf = Vec::new();
    let mut file = std::fs::File::from(read_fd);
    match file.read_to_end(&mut buf) {
        Ok(_) => Some(buf),
        Err(e) => {
            warn!("clipboard offer read error: {e}");
            None
        }
    }
}

/// Create a pipe, returning (read_fd, write_fd).
fn pipe() -> Option<(OwnedFd, OwnedFd)> {
    match rustix::pipe::pipe() {
        Ok((read_fd, write_fd)) => Some((read_fd, write_fd)),
        Err(e) => {
            warn!("pipe() failed: {e}");
            None
        }
    }
}

/// Write data to a file descriptor, taking ownership via dup to avoid double-free.
///
/// The `fd` is a borrowed fd from wayland-client event data. We dup it to get our
/// own owned fd, then write to it.
fn write_to_fd(fd: &impl AsFd, data: &[u8]) -> std::io::Result<()> {
    let owned = fd.as_fd().try_clone_to_owned().map_err(|e| {
        std::io::Error::other(format!("dup failed: {e}"))
    })?;
    let mut file = std::fs::File::from(owned);
    file.write_all(data)
}

// ── Wayland Dispatch implementations ──

impl Dispatch<wl_registry::WlRegistry, ()> for MonitorState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" => {
                    let seat = registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(1), qh, ());
                    state.seat = Some(seat);
                    debug!("bound wl_seat v{version}");
                }
                "zwlr_data_control_manager_v1" => {
                    let manager = registry
                        .bind::<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, _, _>(
                            name,
                            version.min(2),
                            qh,
                            (),
                        );
                    state.manager = Some(manager);
                    debug!("bound zwlr_data_control_manager_v1 v{version}");
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for MonitorState {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // We don't need to handle seat capability events
    }
}

impl Dispatch<zwlr_data_control_manager_v1::ZwlrDataControlManagerV1, ()> for MonitorState {
    fn event(
        _state: &mut Self,
        _manager: &zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
        _event: zwlr_data_control_manager_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // Manager has no events
    }
}

impl Dispatch<zwlr_data_control_device_v1::ZwlrDataControlDeviceV1, ()> for MonitorState {
    fn event(
        state: &mut Self,
        _device: &zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(id, OfferData::default());
            }
            zwlr_data_control_device_v1::Event::Selection { id } => {
                // Self-copy detection
                if state.skip_next_selection {
                    state.skip_next_selection = false;
                    if let Some(offer) = id {
                        offer.destroy();
                        state.offers.remove(&offer);
                    }
                    return;
                }

                if let Some(offer) = id {
                    if let Some(offer_data) = state.offers.get(&offer)
                        && let Some(entry) = read_offer(&offer, offer_data, &state.cfg, conn)
                    {
                        let is_dup = state
                            .history
                            .peek_first()
                            .is_some_and(|first| first == entry);

                        if !is_dup {
                            let _ = state.clip_tx.try_send(entry);
                        }
                    }
                    offer.destroy();
                    state.offers.remove(&offer);
                } else {
                    debug!("clipboard selection cleared");
                }
            }
            zwlr_data_control_device_v1::Event::PrimarySelection {
                id: Some(offer),
            } => {
                // We only care about the regular clipboard, not primary selection
                offer.destroy();
                state.offers.remove(&offer);
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id: None } => {}

            zwlr_data_control_device_v1::Event::Finished => {
                info!("data control device finished");
                state.device = None;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, ()> for MonitorState {
    fn event(
        state: &mut Self,
        offer: &zwlr_data_control_offer_v1::ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some(data) = state.offers.get_mut(offer)
        {
            data.mime_types.push(mime_type);
        }
    }
}

impl Dispatch<zwlr_data_control_source_v1::ZwlrDataControlSourceV1, SourceData> for MonitorState {
    fn event(
        state: &mut Self,
        source: &zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        data: &SourceData,
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd } => {
                let result = match &data.entry {
                    ClipEntry::Text { content } => write_to_fd(&fd, content.as_bytes()),
                    ClipEntry::Image { path, mime } => {
                        // Only serve data if the requested MIME matches what we offered
                        if mime_type != *mime {
                            debug!("ignoring Send for non-matching MIME {mime_type} (have {mime})");
                            return;
                        }
                        std::fs::read(path).and_then(|bytes| write_to_fd(&fd, &bytes))
                    }
                };
                if let Err(e) = result {
                    warn!("failed to send clipboard data: {e}");
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                source.destroy();
                state._active_source = None;
                debug!("clipboard source cancelled");
            }
            _ => {}
        }
    }
}

/// Selects the best MIME type for text from a list of offered types.
#[cfg(test)]
fn best_text_mime(offered: &[String]) -> Option<&'static str> {
    TEXT_MIMES
        .iter()
        .find(|&&mime| offered.iter().any(|o| o == mime))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_text_mime_prefers_utf8() {
        let offered = vec![
            "text/plain".to_string(),
            "text/plain;charset=utf-8".to_string(),
            "UTF8_STRING".to_string(),
        ];
        assert_eq!(best_text_mime(&offered), Some("text/plain;charset=utf-8"));
    }

    #[test]
    fn best_text_mime_falls_back() {
        let offered = vec!["STRING".to_string()];
        assert_eq!(best_text_mime(&offered), Some("STRING"));
    }

    #[test]
    fn best_text_mime_none_for_images_only() {
        let offered = vec!["image/png".to_string()];
        assert_eq!(best_text_mime(&offered), None);
    }

    #[test]
    fn pipe_creates_valid_fds() {
        use std::os::fd::AsRawFd;
        let (read_fd, write_fd) = pipe().expect("pipe should succeed");
        assert!(read_fd.as_raw_fd() >= 0);
        assert!(write_fd.as_raw_fd() >= 0);
    }

    #[test]
    fn best_text_mime_empty_list() {
        let offered: Vec<String> = vec![];
        assert_eq!(best_text_mime(&offered), None);
    }

    #[test]
    fn write_to_fd_roundtrip() {
        let (read_fd, write_fd) = pipe().expect("pipe should succeed");
        let data = b"hello clipboard";
        write_to_fd(&write_fd, data).expect("write should succeed");
        drop(write_fd);

        let mut buf = Vec::new();
        let mut file = std::fs::File::from(read_fd);
        file.read_to_end(&mut buf).expect("read should succeed");
        assert_eq!(buf, data);
    }

    #[test]
    fn write_to_fd_large_payload() {
        let (read_fd, write_fd) = pipe().expect("pipe should succeed");
        // Write 64KB to exercise buffered writes
        let data = vec![0xABu8; 65536];
        // Need to write in a thread since pipe buffer may block
        let data_clone = data.clone();
        let handle = std::thread::spawn(move || {
            write_to_fd(&write_fd, &data_clone).expect("write should succeed");
        });

        let mut buf = Vec::new();
        let mut file = std::fs::File::from(read_fd);
        file.read_to_end(&mut buf).expect("read should succeed");
        handle.join().unwrap();
        assert_eq!(buf.len(), 65536);
        assert_eq!(buf, data);
    }

    #[test]
    fn pipe_read_write_data_integrity() {
        let (read_fd, write_fd) = pipe().expect("pipe should succeed");
        let message = "multi\nline\nclipboard\ncontent\n";
        write_to_fd(&write_fd, message.as_bytes()).expect("write should succeed");
        drop(write_fd);

        let mut buf = Vec::new();
        let mut file = std::fs::File::from(read_fd);
        file.read_to_end(&mut buf).expect("read should succeed");
        assert_eq!(String::from_utf8(buf).unwrap(), message);
    }
}
