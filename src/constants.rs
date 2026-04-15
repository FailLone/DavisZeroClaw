pub const DEFAULT_WINDOW_MINUTES: i64 = 60;
pub const CONTROL_FAILURE_THRESHOLD: usize = 3;
pub const CONTROL_FAILURE_WINDOW_HOURS: i64 = 24;
pub const USER_AGENT: &str = "curl/8.7.1";

pub(crate) const CONTROL_DOMAINS: &[&str] = &[
    "light",
    "switch",
    "cover",
    "fan",
    "climate",
    "lock",
    "scene",
    "script",
    "media_player",
    "humidifier",
    "vacuum",
    "valve",
];

pub(crate) const ROOM_LIGHT_KEYWORDS: &[&str] =
    &["灯", "灯带", "灯光", "吊灯", "主灯", "射灯", "柜灯", "筒灯"];
