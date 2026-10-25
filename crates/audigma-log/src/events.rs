//! Stable event name constants according to the initial catalog (`StructuredLogging.md §10`).

pub const DAEMON_START: &str = "daemon_start";
pub const DAEMON_STOP: &str = "daemon_stop";
pub const DEVICE_LOCK: &str = "device_lock";
pub const RT_PRIORITY: &str = "rt_priority";
pub const CONTROL_SOCKET_BOUND: &str = "control_socket_bound";
pub const OUTPUT_DEVICE_OPENED: &str = "output_device_opened";
pub const OUTPUT_DEVICE_DISCONNECTED: &str = "output_device_disconnected";
pub const DEVICE_WAIT_TICK: &str = "device_wait_tick";
pub const HARDWARE_MIXER_UNAVAILABLE: &str = "hardware_mixer_unavailable";
pub const VOLUME_MODE: &str = "volume_mode";
pub const PW_VOLUME_EVENT: &str = "pw_volume_event";
pub const PW_FORMAT_NEGOTIATED: &str = "pw_format_negotiated";
pub const RATE_SWITCH: &str = "rate_switch";
pub const OUTPUT_REOPENED: &str = "output_reopened";
pub const STREAMING_ERROR: &str = "streaming_error";
pub const DECIMATION_ERROR_SUMMARY: &str = "decimation_error_summary";
pub const RT_EVENTS_DROPPED: &str = "rt_events_dropped";
