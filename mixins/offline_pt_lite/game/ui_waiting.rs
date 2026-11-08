fn offline_pt_lite_ui_waiting(
    game: &crate::game::Game<'_>,
) -> std::ops::ControlFlow<bool> {
    if let Some(waiting) = crate::game::offline_pt_ui::waiting(game) {
        std::ops::ControlFlow::Break(waiting)
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
