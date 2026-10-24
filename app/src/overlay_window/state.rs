use crate::connection::ConnectionTask;
use crate::device_discovery::DiscoveredDevice;
use crate::keyboard::Keyboard;
use crate::protocols::{ConnectionSpec, KeyboardDefinition, Reopener};
use crate::settings::{ProtocolType, Settings};

use egui_file_dialog::FileDialog;
use std::sync::Arc;
use std::time::Instant;

pub enum AppConnectionState {
    Disconnected,
    /// Shared so the overlay can draw and the editor can write through the same
    /// `Keyboard` while other UI code mutates app state.
    Connected {
        keyboard: Arc<Keyboard>,
    },
    /// Foxglove (#279): a burst of tries started by an event (the link dropped, the device
    /// arrived), stopped by connecting or after `RECONNECT_TRIES`; then `None`, waiting for the
    /// next device arrival instead of retrying forever.
    Reconnecting {
        next_attempt_at: Option<Instant>,
        tries: u8,
    },
}

impl AppConnectionState {
    /// A new burst of tries, starting now.
    pub fn reconnect_now() -> Self {
        AppConnectionState::Reconnecting { next_attempt_at: Some(Instant::now()), tries: 0 }
    }
}

#[derive(Clone)]
pub enum ZmkTransportDraft {
    Serial { port_name: Option<String> },
    Ble { device_id: Option<String> },
}

#[derive(Clone)]
pub enum ConnectionDraft {
    Via { json_path: String },
    Vial,
    Zmk { transport: ZmkTransportDraft },
    Mock,
}

impl ConnectionDraft {
    pub fn protocol_type(&self) -> ProtocolType {
        match self {
            ConnectionDraft::Via { .. } => ProtocolType::Via,
            ConnectionDraft::Vial => ProtocolType::Vial,
            ConnectionDraft::Zmk { .. } => ProtocolType::Zmk,
            ConnectionDraft::Mock => ProtocolType::Via,
        }
    }
}

pub struct UiState {
    pub settings_visible: bool,
    pub settings_error: Option<String>,
    pub settings_warning: Option<String>,
    pub mouse_passthrough: Option<bool>,
    pub file_dialog: FileDialog,
}

pub struct SettingsState {
    pub active: Settings,
    pub draft: Settings,
}

pub struct SessionState {
    pub connection: AppConnectionState,
    pub ever_connected: bool,
    pub last_spec: Option<ConnectionSpec>,
    pub reopen: Option<Arc<dyn Reopener>>,
    pub connected_definition: Option<KeyboardDefinition>,
    pub layout_names: Vec<String>,
    pub active_layout_name: String,
    pub draft_layout_name: String,
}

pub struct ConnectDraftState {
    pub available_devices: Vec<DiscoveredDevice>,
    pub selected_device_index: Option<usize>,
    pub draft: ConnectionDraft,
    pub pending_connect: Option<ConnectionTask>,
}
