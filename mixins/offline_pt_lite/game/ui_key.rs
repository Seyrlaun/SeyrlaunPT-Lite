fn offline_pt_lite_ui_key(
    game: &mut crate::game::Game<'_>,
    key: winit::keyboard::KeyCode,
) -> std::ops::ControlFlow<Result<()>> {
    if crate::game::offline_pt_ui::settings_key(game, key) {
        std::ops::ControlFlow::Break(Ok(()))
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
