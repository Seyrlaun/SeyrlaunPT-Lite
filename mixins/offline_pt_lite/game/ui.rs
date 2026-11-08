use super::{Game, Screen};
use horizon_client::{
    chat::ChatSource, hud::HudBatch, keybinds, native_pt_config::{self, PtOptions},
    text::Text, ui,
};
use std::sync::Mutex;
use winit::{event::MouseButton, keyboard::KeyCode};
const ID: &str = "offline_pt_lite";
const TOGGLE: &str = "toggle";
const DEFAULT_KEY: &str = "key.keyboard.f6";
const CONTROLS: usize = 9;
#[derive(Clone, Copy)]
struct PageState {
    under: Screen,
    binding: bool,
}
static PAGE: Mutex<Option<PageState>> = Mutex::new(None);
fn page() -> Option<PageState> {
    *PAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
pub(super) fn reset() {
    *PAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
pub(super) fn active(game: &Game<'_>) -> bool {
    game.screen == Screen::Options && page().is_some()
}
pub(super) fn waiting(game: &Game<'_>) -> Option<bool> {
    active(game).then(|| page().is_some_and(|page| page.binding))
}
fn toggle_key<'a>(game: &'a Game<'_>) -> &'a str {
    game.settings
        .mods
        .keys
        .get(ID)
        .and_then(|keys| keys.get(TOGGLE))
        .map(String::as_str)
        .filter(|key| *key != "key.keyboard.f8")
        .unwrap_or(DEFAULT_KEY)
}
fn chat(game: &mut Game<'_>, text: &str) {
    game.ui
        .chat
        .add(
            &game.gui.atlas,
            Text::literal(text),
            None,
            ChatSource::Client,
            game.ticks,
            game.screen == Screen::Chat,
        );
}
pub(super) fn capture_busy_message(game: &mut Game<'_>) {
    chat(game, "PT screenshot is still processing.");
}
fn toggle(game: &mut Game<'_>) {
    let options = native_pt_config::update(|options| options.enabled = !options.enabled);
    chat(game, if options.enabled { "Path tracing: ON" } else { "Path tracing: OFF" });
}
fn open(game: &mut Game<'_>) {
    *PAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PageState {
        under: game.screen,
        binding: false,
    });
    game.keys.clear();
    game.dragging = false;
    game.screen = Screen::Options;
    game.capture(false);
}
fn close(game: &mut Game<'_>) {
    let state = PAGE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(state) = state {
        game.dragging = false;
        game.screen = state.under;
        if game.screen == Screen::Playing && game.player.is_some() {
            game.capture(true);
        }
    }
}
pub(super) fn key(game: &mut Game<'_>, key: KeyCode) -> bool {
    if active(game) {
        return settings_key(game, key);
    }
    if game.title.is_some() || game.player.is_none() || game.options.dedicated.is_some()
        || game.sign_edit.is_some() || game.book.is_some()
    {
        return false;
    }
    if key == KeyCode::F8 && matches!(game.screen, Screen::Playing | Screen::Paused) {
        open(game);
        return true;
    }
    if game.screen == Screen::Playing && game.sleeping.is_none()
        && keybinds::key_name(key).is_some_and(|key| key == toggle_key(game))
    {
        toggle(game);
    }
    false
}
pub(super) fn mouse_toggle(game: &mut Game<'_>, button: MouseButton, pressed: bool) {
    if pressed && game.screen == Screen::Playing && game.title.is_none()
        && game.player.is_some() && game.sleeping.is_none()
        && game.options.dedicated.is_none() && game.sign_edit.is_none()
        && game.book.is_none() && keybinds::mouse_name(button) == toggle_key(game)
    {
        toggle(game);
    }
}
fn bind(game: &mut Game<'_>, key: String) {
    if key == "key.keyboard.f8" {
        return;
    }
    game.settings.mods.keys.entry(ID.into()).or_default().insert(TOGGLE.into(), key);
    game.settings.save(&Game::options_path());
    if let Some(state) = PAGE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_mut()
    {
        state.binding = false;
    }
}
pub(super) fn settings_key(game: &mut Game<'_>, key: KeyCode) -> bool {
    if !active(game) {
        return false;
    }
    if page().is_some_and(|page| page.binding) {
        let key = if key == KeyCode::Escape {
            "key.keyboard.unknown"
        } else {
            keybinds::key_name(key).unwrap_or("key.keyboard.unknown")
        };
        bind(game, key.into());
    } else if matches!(key, KeyCode::Escape | KeyCode::F8) {
        close(game);
    }
    true
}
pub(super) fn bind_mouse(game: &mut Game<'_>, button: MouseButton) -> bool {
    if !active(game) {
        return false;
    }
    if page().is_some_and(|page| page.binding) {
        bind(game, keybinds::mouse_name(button));
    }
    true
}
fn control_bounds(width: f32, index: usize) -> [f32; 4] {
    let column = ((width - 32.0) / 2.0).clamp(40.0, 150.0);
    let left = (width / 2.0).floor() - column - 4.0;
    [
        left + (index % 2) as f32 * (column + 8.0),
        32.0 + (index / 2) as f32 * 24.0,
        column,
        20.0,
    ]
}
fn done_bounds(width: f32, height: f32) -> [f32; 4] {
    let button = (width - 24.0).clamp(40.0, 200.0);
    [(width / 2.0).floor() - button / 2.0, height - 27.0, button, 20.0]
}
fn labels(options: PtOptions, key: &str, binding: bool) -> [String; CONTROLS] {
    let on_off = |value| if value { "ON" } else { "OFF" };
    let key = keybinds::display_name(key).plain();
    [
        if binding { "> Toggle key <".into() } else { format!("Toggle: {key}") },
        format!("Path tracing: {}", on_off(options.enabled)),
        format!("Resolution: {}%", options.scale),
        format!("Bounces: {}", options.bounces),
        format!("Freeze scene: {}", on_off(options.freeze)),
        format!("Exposure: {:+.1} EV", options.exposure as f32 / 10.),
        "Exposure -0.5 EV".into(),
        "Exposure +0.5 EV".into(),
        format!("F2 OIDN: {}", on_off(options.denoise)),
    ]
}
pub(super) fn draw(game: &mut Game<'_>, batch: &mut HudBatch) -> bool {
    if !active(game) {
        return false;
    }
    let atlas = &game.gui.atlas;
    ui::menu_background(batch, atlas, 0.0, 0.0, batch.width, batch.height, true);
    ui::centered_text(
        batch,
        atlas,
        &Text::literal("Seyrlaun Lite 0.1.0-alpha.3"),
        0.0,
        13.0,
        batch.width,
        u32::MAX,
    );
    let options = native_pt_config::snapshot();
    let binding = page().is_some_and(|page| page.binding);
    for (index, label) in labels(options, toggle_key(game), binding)
        .into_iter()
        .enumerate()
    {
        let [x, y, width, height] = control_bounds(batch.width, index);
        ui::button(
            batch,
            atlas,
            x,
            y,
            width,
            height,
            &Text::literal(label),
            true,
            ui::hovered(game.cursor, x, y, width, height),
        );
    }
    let [x, y, width, height] = done_bounds(batch.width, batch.height);
    ui::button(
        batch,
        atlas,
        x,
        y,
        width,
        height,
        &Text::literal("Done"),
        true,
        ui::hovered(game.cursor, x, y, width, height),
    );
    true
}
pub(super) fn mouse(game: &mut Game<'_>, pressed: bool, right: bool) -> bool {
    if !active(game) {
        return false;
    }
    if !pressed || right {
        return true;
    }
    let (width, height) = game.gui_size();
    let cursor = game.cursor;
    let [x, y, w, h] = done_bounds(width, height);
    if ui::hovered(cursor, x, y, w, h) {
        game.audio.play_button_click();
        close(game);
        return true;
    }
    let selected = (0..CONTROLS)
        .find(|&index| {
            let [x, y, width, height] = control_bounds(width, index);
            ui::hovered(cursor, x, y, width, height)
        });
    let Some(selected) = selected else {
        return true;
    };
    game.audio.play_button_click();
    if selected == 0 {
        if let Some(state) = PAGE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            state.binding = true;
        }
        return true;
    }
    native_pt_config::update(|options| match selected {
        1 => options.enabled = !options.enabled,
        2 => {
            options.scale = match options.scale {
                100 => 75,
                75 => 50,
                50 => 25,
                _ => 100,
            };
        }
        3 => options.bounces = options.bounces % 8 + 1,
        4 => options.freeze = !options.freeze,
        5 => options.exposure = 0,
        6 => options.exposure = options.exposure.saturating_sub(5),
        7 => options.exposure = options.exposure.saturating_add(5),
        8 => options.denoise = !options.denoise,
        _ => {}
    });
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_labels_expose_every_preserved_lite_option() {
        let controls = labels(PtOptions::default(), DEFAULT_KEY, false);
        assert_eq!(controls.len(), CONTROLS);
        assert_eq!(controls[1], "Path tracing: OFF");
        assert_eq!(controls[2], "Resolution: 100%");
        assert_eq!(controls[3], "Bounces: 4");
        assert_eq!(controls[4], "Freeze scene: ON");
        assert_eq!(controls[5], "Exposure: +0.0 EV");
        assert_eq!(controls[8], "F2 OIDN: ON");
        assert_eq!(
            labels(PtOptions::default(), DEFAULT_KEY, true) [0], "> Toggle key <"
        );
    }
    #[test]
    fn settings_controls_fit_the_smallest_standard_gui_without_overlap() {
        for (width, height) in [(320.0, 180.0), (640.0, 360.0)] {
            let done = done_bounds(width, height);
            for index in 0..CONTROLS {
                let [x, y, w, h] = control_bounds(width, index);
                assert!(x >= 0.0 && x + w <= width);
                assert!(y >= 28.0 && y + h < done[1]);
                for other in 0..index {
                    let [ox, oy, ow, oh] = control_bounds(width, other);
                    assert!(x + w <= ox || ox + ow <= x || y + h <= oy || oy + oh <= y);
                }
            }
        }
    }
}
