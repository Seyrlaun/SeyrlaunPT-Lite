use super::*;
use horizon_client::native_pt_config;
const ID: &str = "offline_pt_lite";
const TOGGLE: &str = "toggle";
const DEFAULT_KEY: &str = "key.keyboard.f6";
pub(super) fn register(_game: &mut Game<'_>, mods: &mut Vec<NativeMod>) {
    mods.push(NativeMod {
        id: ID.into(),
        title: "Offline PT Lite 0.1.0-alpha.2".into(),
        description: "Progressive world-space path tracing.".into(),
        keys: vec![[TOGGLE.into(), "Toggle path tracing".into(), DEFAULT_KEY.into()]],
    });
}
pub(super) fn key(game: &mut Game<'_>, key: &str, pressed: bool) {
    if !pressed || !game.native_on(ID) || !game.native_playing()
        || key != game.native_mod_key(ID, TOGGLE, DEFAULT_KEY)
    {
        return;
    }
    let options = native_pt_config::update(|options| options.enabled = !options.enabled);
    game.native_chat(
        if options.enabled { "Path tracing: ON" } else { "Path tracing: OFF" },
    );
}
pub(super) fn settings(_game: &mut Game<'_>, mod_id: &str, buttons: &mut Vec<String>) {
    if mod_id != ID {
        return;
    }
    let options = native_pt_config::snapshot();
    let on_off = |value| if value { "ON" } else { "OFF" };
    buttons
        .extend([
            format!("Path tracing: {}", on_off(options.enabled)),
            format!("Resolution: {}%", options.scale),
            format!("Bounces: {}", options.bounces),
            format!("Freeze scene: {}", on_off(options.freeze)),
            format!("Exposure: {:+.1} EV", options.exposure as f32 / 10.),
            "Exposure -0.5 EV".into(),
            "Exposure +0.5 EV".into(),
            format!("F2 OIDN: {}", on_off(options.denoise)),
        ]);
}
pub(super) fn click(_game: &mut Game<'_>, mod_id: &str, button: usize) {
    if mod_id != ID || button > 7 {
        return;
    }
    native_pt_config::update(|options| match button {
        0 => options.enabled = !options.enabled,
        1 => {
            options.scale = match options.scale {
                100 => 75,
                75 => 50,
                50 => 25,
                _ => 100,
            };
        }
        2 => options.bounces = options.bounces % 8 + 1,
        3 => options.freeze = !options.freeze,
        4 => options.exposure = 0,
        5 => options.exposure = options.exposure.saturating_sub(5),
        6 => options.exposure = options.exposure.saturating_add(5),
        7 => options.denoise = !options.denoise,
        _ => {}
    });
}
