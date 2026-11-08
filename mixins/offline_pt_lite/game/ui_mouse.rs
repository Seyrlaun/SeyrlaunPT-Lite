fn offline_pt_lite_ui_mouse(
    game: &mut crate::game::Game<'_>,
    pressed: bool,
    right: bool,
) -> std::ops::ControlFlow<Result<()>> {
    if crate::game::offline_pt_ui::mouse(game, pressed, right) {
        std::ops::ControlFlow::Break(Ok(()))
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
