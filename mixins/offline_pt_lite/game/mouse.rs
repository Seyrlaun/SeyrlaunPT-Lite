fn offline_pt_lite_mouse(
    game: &mut crate::game::Game<'_>,
    button: winit::event::MouseButton,
    pressed: bool,
) {
    crate::game::offline_pt_ui::mouse_toggle(game, button, pressed);
}
