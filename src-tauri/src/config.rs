use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// All stored geometry is in PHYSICAL pixels.
/// WebView2 renders crisply at any DPI scale, so we never fight the system
/// scale factor -- we only need positions that survive a scale change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    #[serde(default = "yes")]
    pub visible: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    pub label: String,
    #[serde(default = "default_tz")]
    pub timezone: String,
    /// ISO country code and first-level division code (GeoNames admin1) the
    /// settings page picked this place from, so it can re-open on the same
    /// entries. Empty for hand-edited or older configs.
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub admin1: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wallpaper {
    /// "behind_icons" re-parents into the shell's wallpaper layer so desktop
    /// icons stay on top. "bottom" just sinks the window to the bottom of the
    /// z-order (icons will be covered, but it never touches the shell).
    #[serde(default = "default_layer")]
    pub layer: String,
    /// What the camera's longitude is pinned to.
    /// "location" keeps your own city centred and lets the terminator sweep
    /// across it; "sun" keeps the lit face centred and lets the continents
    /// rotate past; "fixed" treats `camera_offset_deg` as an absolute
    /// longitude and never moves.
    #[serde(default = "default_anchor")]
    pub camera_anchor: String,
    /// Degrees of longitude east of the anchor.
    /// 0 keeps the lit face centred; the default swings it slightly so the
    /// terminator stays visible.
    #[serde(default = "default_camera_offset")]
    pub camera_offset_deg: f64,
    /// Camera latitude, degrees. Positive tilts the north pole toward us.
    #[serde(default = "default_camera_lat")]
    pub camera_lat_deg: f64,
    /// Distance from the globe centre, in Earth radii. Larger flattens the
    /// perspective toward an orthographic "from deep space" look.
    #[serde(default = "default_distance")]
    pub camera_distance: f64,
    /// How far the camera pitches up from straight down, toward the horizon.
    #[serde(default = "default_tilt")]
    pub camera_tilt_deg: f64,
    /// Rotation about the vertical before tilting; positive turns the view
    /// clockwise (north drifts left).
    #[serde(default = "default_heading")]
    pub camera_heading_deg: f64,
    /// Focal length in half-heights of the framing monitor. Larger zooms in.
    #[serde(default = "default_focal")]
    pub focal_length: f64,
    /// Where the camera's optical axis meets the framing monitor, as an
    /// offset from its centre in half-heights (x right, y down). The defaults
    /// put it below centre, so the limb arcs across the upper half.
    #[serde(default = "default_axis_x")]
    pub axis_offset_x: f64,
    #[serde(default = "default_axis_y")]
    pub axis_offset_y: f64,
    /// Which screen the framing below is measured against.
    ///
    /// The wallpaper window covers the whole virtual desktop, so on a
    /// multi-monitor setup "virtual" centres the globe in the middle of all
    /// the monitors put together -- usually a gap between two of them.
    /// "primary" measures against the primary monitor instead, which is
    /// almost always what you want.
    #[serde(default = "default_frame")]
    pub frame_monitor: String,
    /// Give every monitor a globe of its own, each composed like the primary
    /// (offsets and focal length measured against that monitor's height).
    /// Off: one scene spans all monitors, framed on `frame_monitor`.
    #[serde(default = "yes")]
    pub each_monitor: bool,
    /// Which day texture: "apple" (default) is graded toward Apple's Earth
    /// wallpaper -- richer land, ochre deserts, muted navy-to-teal ocean,
    /// baked valley shading; "calm" is plain Blue Marble with an even ocean;
    /// "relief" is plain Blue Marble with its sea-floor shading.
    #[serde(default = "default_ocean_style")]
    pub ocean_style: String,
    /// Procedural detail layered onto the live cloud map, 0..1.
    #[serde(default = "one")]
    pub cloud_detail: f64,
    /// Render at a fraction of the native resolution. 4K is a lot of pixels
    /// to shade every frame; 0.75 is usually indistinguishable.
    #[serde(default = "default_render_scale")]
    pub render_scale: f64,
    /// Extra spin, degrees per hour, on top of real Earth rotation. 0 = real.
    #[serde(default)]
    pub drift_deg_per_hour: f64,
    #[serde(default = "default_clouds")]
    pub cloud_opacity: f64,
    /// Use today's real cloud cover (a global map rebuilt every few hours
    /// from geostationary satellites) instead of the bundled static one.
    /// Falls back to the bundled map whenever the download fails.
    #[serde(default = "yes")]
    pub live_clouds: bool,
    #[serde(default = "default_live_clouds_url")]
    pub live_clouds_url: String,
    #[serde(default = "default_city_lights")]
    pub city_lights: f64,
    /// Brightness of the real starfield behind the planet. 0 turns it off.
    /// The stars are catalogue positions, not a texture, so they wheel with
    /// sidereal time and sit where they actually are.
    #[serde(default = "default_stars")]
    pub stars: f64,
    /// Brightness of the sun's disc and glare when it is in frame. 0 hides it.
    #[serde(default = "one")]
    pub sun: f64,
    /// Brightness of the moon. It is drawn at its real position, distance and
    /// phase, so it is often nowhere near the screen. 0 hides it.
    #[serde(default = "one")]
    pub moon: f64,
    /// Cap the render loop. The globe turns a quarter of a degree a minute,
    /// so 15 is already far more than the motion needs.
    #[serde(default = "default_fps")]
    pub max_fps: u32,
    /// Seconds between wallpaper frames. The globe is not animated: it is
    /// redrawn only when the clock has moved enough to matter (and at once on
    /// a new cloud map or a monitor change), so between frames it costs no
    /// more than a static wallpaper. `max_fps` is no longer used.
    #[serde(default = "default_redraw")]
    pub redraw_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sensors {
    /// Fallback only. Sensors come from EarthDesk's own hardware monitor
    /// service over a named pipe; this external LibreHardwareMonitor web
    /// server is asked only when that service does not answer.
    #[serde(default = "default_lhm")]
    pub lhm_url: String,
    #[serde(default = "default_sample_ms")]
    pub sample_ms: u64,
    /// Which mainboard fan header feeds what: "Fan #2" -> "cpu" | "case".
    /// Most boards only label them "Fan #n", so the performance widget
    /// learns this by itself -- the headers whose speed climbs with the CPU
    /// temperature are the CPU cooler -- and saves it here. Edit by hand to
    /// override; delete to make it learn again.
    #[serde(default)]
    pub fan_roles: BTreeMap<String, String>,
}

/// Bump whenever a default changes in a way that a stale config file would
/// visibly override. The file on disk wins over code defaults -- that is the
/// point of a config file -- so without this, the very first run's defaults
/// are frozen in for good and every later change looks like it did nothing.
pub const CURRENT_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub version: u32,
    /// Where the widgets live in the z-order.
    ///
    /// "desktop"  — parked inside the wallpaper window, so they ride in the
    ///              desktop layer: over the globe, under the desktop icons,
    ///              and under every application window. This is what makes
    ///              them feel like part of the desktop.
    /// "bottom"   — bottom of the ordinary z-order. Same practical effect,
    ///              without reparenting anything; the fallback if "desktop"
    ///              misbehaves.
    /// "topmost"  — float above everything, including full-screen windows.
    /// "normal"   — ordinary windows, wherever the z-order puts them.
    #[serde(default = "default_z")]
    pub z_mode: String,
    /// Multiplies the size of everything inside the widgets. The layout
    /// already follows the system DPI scale; this is for when you want it
    /// bigger (or smaller) than the system default on top of that.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f64,
    /// How opaque the widgets' tinted card is: 0 is fully see-through (text
    /// only), 1 is solid. Set from the settings page.
    #[serde(default = "default_card_opacity")]
    pub card_opacity: f64,
    #[serde(default = "default_threshold")]
    pub snap_threshold: i32,
    #[serde(default = "default_hotkey")]
    pub edit_hotkey: String,
    pub location: Location,
    #[serde(default = "default_weather_minutes")]
    pub weather_refresh_minutes: u64,
    #[serde(default)]
    pub wallpaper: Wallpaper,
    #[serde(default)]
    pub sensors: Sensors,
    /// The settings page's switchboard. Keys are "wallpaper", "weather" and
    /// "perf" for whole pieces, and "<piece>.<part>" for the parts inside
    /// them ("wallpaper.stars", "perf.gpu", ...). A key that is missing means
    /// on, so a feature added later starts out enabled without migrating
    /// anyone's file.
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    pub widgets: BTreeMap<String, WidgetRect>,
}

fn default_z() -> String {
    "desktop".into()
}
fn default_ui_scale() -> f64 {
    1.0
}
fn default_card_opacity() -> f64 {
    0.85
}
fn default_threshold() -> i32 {
    12
}
fn default_hotkey() -> String {
    "Ctrl+Alt+D".into()
}
fn default_tz() -> String {
    "Asia/Tokyo".into()
}
fn default_layer() -> String {
    "behind_icons".into()
}
fn default_anchor() -> String {
    "location".into()
}
fn default_camera_offset() -> f64 {
    -0.106
}
fn default_distance() -> f64 {
    3.213
}
fn default_render_scale() -> f64 {
    1.0
}
fn default_tilt() -> f64 {
    0.0
}
fn default_heading() -> f64 {
    0.186
}
fn default_focal() -> f64 {
    5.166
}
fn default_axis_x() -> f64 {
    -0.0006
}
fn default_axis_y() -> f64 {
    1.541
}
fn default_stars() -> f64 {
    0.4
}
fn default_live_clouds_url() -> String {
    "https://clouds.matteason.co.uk/images/8192x4096/clouds.jpg".into()
}
fn default_frame() -> String {
    "primary".into()
}
fn default_ocean_style() -> String {
    "apple".into()
}
fn default_camera_lat() -> f64 {
    0.843
}
fn default_clouds() -> f64 {
    0.9
}
fn default_city_lights() -> f64 {
    1.0
}
fn default_redraw() -> u32 {
    20
}
fn one() -> f64 {
    1.0
}
fn default_fps() -> u32 {
    15
}
fn default_lhm() -> String {
    "http://127.0.0.1:8085/data.json".into()
}
fn default_sample_ms() -> u64 {
    2000
}
fn default_weather_minutes() -> u64 {
    10
}

impl Wallpaper {
    /// The first camera was fitted to an early tablet screenshot whose
    /// landmarks were misread (Hainan taken for Taiwan), which made the view
    /// half again too close. The 2026-09-22 refit against the user's own iPad
    /// home screen puts the camera above the equator at the home longitude,
    /// ~3.2 radii out, with a long lens aimed at the northern limb -- the
    /// same framing Apple uses. Configs still carrying the old fitted values
    /// untouched are moved to the new ones; anything hand-edited is left alone.
    pub fn upgrade_camera(&mut self) -> bool {
        let old = (self.camera_lat_deg, self.camera_distance, self.focal_length, self.axis_offset_y);
        if old != (19.0, 1.42, 1.747, 0.546) {
            return false;
        }
        let d = Wallpaper::default();
        self.camera_offset_deg = d.camera_offset_deg;
        self.camera_lat_deg = d.camera_lat_deg;
        self.camera_distance = d.camera_distance;
        self.camera_tilt_deg = d.camera_tilt_deg;
        self.camera_heading_deg = d.camera_heading_deg;
        self.focal_length = d.focal_length;
        self.axis_offset_x = d.axis_offset_x;
        self.axis_offset_y = d.axis_offset_y;
        true
    }
}

impl Default for Wallpaper {
    fn default() -> Self {
        Wallpaper {
            layer: default_layer(),
            camera_anchor: default_anchor(),
            camera_offset_deg: default_camera_offset(),
            camera_distance: default_distance(),
            camera_tilt_deg: default_tilt(),
            camera_heading_deg: default_heading(),
            focal_length: default_focal(),
            axis_offset_x: default_axis_x(),
            axis_offset_y: default_axis_y(),
            frame_monitor: default_frame(),
            each_monitor: true,
            ocean_style: default_ocean_style(),
            cloud_detail: one(),
            render_scale: default_render_scale(),
            camera_lat_deg: default_camera_lat(),
            drift_deg_per_hour: 0.0,
            cloud_opacity: default_clouds(),
            live_clouds: true,
            live_clouds_url: default_live_clouds_url(),
            city_lights: default_city_lights(),
            stars: default_stars(),
            sun: one(),
            moon: one(),
            max_fps: default_fps(),
            redraw_seconds: default_redraw(),
        }
    }
}

impl Default for Sensors {
    fn default() -> Self {
        Sensors { lhm_url: default_lhm(), sample_ms: default_sample_ms(), fan_roles: BTreeMap::new() }
    }
}

impl Default for Config {
    fn default() -> Self {
        // Deliberately empty: a widget with no entry here is laid out from
        // DEFAULT_LAYOUT in main.rs against the monitor it lands on, so the
        // first run comes out the right apparent size at any DPI scale.
        // Baking pixel sizes in here would make them wrong on a 4K display.
        let widgets = BTreeMap::new();
        Config {
            version: CURRENT_VERSION,
            z_mode: default_z(),
            ui_scale: default_ui_scale(),
            card_opacity: default_card_opacity(),
            snap_threshold: default_threshold(),
            edit_hotkey: default_hotkey(),
            // A fresh install starts in Beijing; the settings page changes it.
            location: Location {
                lat: 39.9042,
                lon: 116.4074,
                label: "北京".into(),
                timezone: "Asia/Shanghai".into(),
                country: "CN".into(),
                admin1: "22".into(),
            },
            weather_refresh_minutes: default_weather_minutes(),
            wallpaper: Wallpaper::default(),
            sensors: Sensors::default(),
            features: BTreeMap::new(),
            widgets,
        }
    }
}

/// `%APPDATA%\EarthDesk\config.json`. Copies made before the rename kept
/// theirs under `OsakaDesk`; that file is moved over on first start.
pub fn config_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let p = base.join("EarthDesk").join("config.json");
    let legacy = base.join("OsakaDesk").join("config.json");
    if !p.exists() && legacy.exists() {
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::copy(&legacy, &p);
    }
    p
}

impl Config {
    pub fn feature(&self, key: &str) -> bool {
        self.features.get(key).copied().unwrap_or(true)
    }

    pub fn load() -> Self {
        let path = config_path();
        let existing: Option<Config> = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(cfg) => Some(cfg),
                Err(e) => {
                    eprintln!("config parse failed ({e}), falling back to defaults");
                    None
                }
            },
            Err(_) => None,
        };

        let config = match existing {
            Some(mut cfg) if cfg.version >= CURRENT_VERSION => {
                if cfg.wallpaper.upgrade_camera() {
                    cfg.save();
                }
                return cfg;
            }
            Some(old) => {
                eprintln!(
                    "config is from version {} (current {CURRENT_VERSION}); \
                     resetting layout and appearance, keeping location and sensors",
                    old.version
                );
                Config {
                    // Things the user is likely to have set on purpose.
                    location: old.location,
                    sensors: old.sensors,
                    edit_hotkey: old.edit_hotkey,
                    weather_refresh_minutes: old.weather_refresh_minutes,
                    snap_threshold: old.snap_threshold,
                    features: old.features,
                    z_mode: old.z_mode,
                    ui_scale: old.ui_scale,
                    card_opacity: old.card_opacity,
                    ..Config::default()
                }
            }
            None => Config::default(),
        };
        config.save();
        config
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    eprintln!("config write failed: {e}");
                }
            }
            Err(e) => eprintln!("config serialise failed: {e}"),
        }
    }
}
