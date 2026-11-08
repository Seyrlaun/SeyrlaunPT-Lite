use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, sync::{Mutex, OnceLock}};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PtOptions {
    pub enabled: bool,
    pub scale: u32,
    pub bounces: u32,
    pub freeze: bool,
    pub exposure: i32,
    pub denoise: bool,
}
impl Default for PtOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            scale: 100,
            bounces: 4,
            freeze: true,
            exposure: 0,
            denoise: true,
        }
    }
}
impl PtOptions {
    fn clamp(&mut self) {
        self.scale = self.scale.clamp(25, 100);
        self.bounces = self.bounces.clamp(1, 8);
        self.exposure = self.exposure.clamp(-40, 40);
    }
}
fn boolean(value: &str) -> Option<bool> {
    match value.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}
fn legacy(text: &str) -> PtOptions {
    let mut options = PtOptions::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        match key.trim() {
            "pathTracing" => {
                if let Some(value) = boolean(value) {
                    options.enabled = value;
                }
            }
            "pathTracingScale" => {
                if let Ok(value) = value.trim().parse() {
                    options.scale = value;
                }
            }
            "pathTracingBounces" => {
                if let Ok(value) = value.trim().parse() {
                    options.bounces = value;
                }
            }
            "pathTracingFreeze" => {
                if let Some(value) = boolean(value) {
                    options.freeze = value;
                }
            }
            "pathTracingExposure" => {
                if let Ok(value) = value.trim().parse() {
                    options.exposure = value;
                }
            }
            _ => {}
        }
    }
    options.clamp();
    options
}
fn home() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
fn path() -> PathBuf {
    home().join("config").join("offline-pt-lite.json")
}
fn load() -> PtOptions {
    let mut options = fs::read_to_string(home().join("horizon-graphics.txt"))
        .map(|text| legacy(&text))
        .unwrap_or_default();
    match fs::read_to_string(path()) {
        Ok(text) => {
            match serde_json::from_str::<PtOptions>(&text) {
                Ok(saved) => options = saved,
                Err(error) => {
                    log::warn!("Offline PT Lite settings could not be read: {error}")
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("Offline PT Lite settings could not be read: {error}"),
    }
    match std::env::var("HORIZON_RENDERER").as_deref() {
        Ok("path") => options.enabled = true,
        Ok("raster") => options.enabled = false,
        _ => {}
    }
    if let Some(value) = std::env::var("HORIZON_PT_SCALE")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
    {
        options.scale = (value * 100.).round().clamp(25., 100.) as u32;
    }
    if let Some(value) = std::env::var("HORIZON_PT_BOUNCES")
        .ok()
        .and_then(|value| value.parse().ok())
    {
        options.bounces = value;
    }
    if let Some(value) = std::env::var("HORIZON_PT_FREEZE")
        .ok()
        .and_then(|value| boolean(&value))
    {
        options.freeze = value;
    }
    if let Some(value) = std::env::var("HORIZON_PT_EXPOSURE_EV")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
    {
        options.exposure = (value * 10.).round().clamp(-40., 40.) as i32;
    }
    options.clamp();
    options
}
fn state() -> &'static Mutex<PtOptions> {
    static STATE: OnceLock<Mutex<PtOptions>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(load()))
}
pub fn snapshot() -> PtOptions {
    *state().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub fn update(change: impl FnOnce(&mut PtOptions)) -> PtOptions {
    let mut options = state().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = *options;
    change(&mut options);
    options.clamp();
    if *options != previous {
        let save = || -> Result<(), Box<dyn std::error::Error>> {
            let path = path();
            fs::create_dir_all(path.parent().ok_or("settings directory is missing")?)?;
            let temporary = path.with_extension("json.tmp");
            fs::write(&temporary, serde_json::to_vec_pretty(&*options)?)?;
            fs::rename(&temporary, &path)?;
            Ok(())
        };
        if let Err(error) = save() {
            log::warn!("Offline PT Lite settings could not be saved: {error}");
        }
    }
    *options
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_migration_reads_only_owned_options_and_clamps_values() {
        let options = legacy(
            "hdr:true\npathTracing:true\npathTracingScale:0\n\
            pathTracingBounces:99\npathTracingFreeze:false\npathTracingExposure:-99\n",
        );
        assert_eq!(
            options, PtOptions { enabled : true, scale : 25, bounces : 8, freeze : false,
            exposure : - 40, denoise : true }
        );
    }
    #[test]
    fn malformed_legacy_values_keep_defaults() {
        assert_eq!(
            legacy("pathTracing:maybe\npathTracingBounces:no\n\
            pathTracingScale:bad\npathTracingExposure:NaN\n"),
            PtOptions::default()
        );
    }
    #[test]
    fn independent_settings_round_trip_and_missing_fields_default() {
        let options = PtOptions {
            enabled: true,
            scale: 75,
            bounces: 6,
            freeze: false,
            exposure: -7,
            denoise: true,
        };
        let json = serde_json::to_string(&options).unwrap();
        assert_eq!(serde_json::from_str::< PtOptions > (& json).unwrap(), options);
        assert_eq!(
            serde_json::from_str::< PtOptions > ("{}").unwrap(), PtOptions::default()
        );
    }
}
