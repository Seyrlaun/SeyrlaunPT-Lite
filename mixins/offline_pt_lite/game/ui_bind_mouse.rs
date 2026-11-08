fn offline_pt_lite_ui_bind_mouse(
    game: &mut crate::game::Game<'_>,
    button: winit::event::MouseButton,
) -> std::ops::ControlFlow<Result<()>> {
    if crate::game::offline_pt_ui::bind_mouse(game, button) {
        std::ops::ControlFlow::Break(Ok(()))
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
