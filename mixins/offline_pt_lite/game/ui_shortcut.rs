fn offline_pt_lite_ui_shortcut(
    game: &mut crate::game::Game<'_>,
) -> std::ops::ControlFlow<bool> {
    if crate::game::offline_pt_ui::active(game) {
        std::ops::ControlFlow::Break(false)
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
