fn offline_pt_lite_ui_drag(
    game: &mut crate::game::Game<'_>,
) -> std::ops::ControlFlow<Result<()>> {
    if crate::game::offline_pt_ui::active(game) {
        std::ops::ControlFlow::Break(Ok(()))
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
