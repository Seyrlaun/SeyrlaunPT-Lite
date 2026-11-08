fn offline_pt_lite_ui_scroll(
    game: &mut crate::game::Game<'_>,
) -> std::ops::ControlFlow<()> {
    if crate::game::offline_pt_ui::active(game) {
        std::ops::ControlFlow::Break(())
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
