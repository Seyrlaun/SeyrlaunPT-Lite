fn offline_pt_lite_screenshot_guard(
    game: &mut crate::game::Game<'_>,
) -> std::ops::ControlFlow<()> {
    if game.ui.screenshot_message.is_some() || game.pending_screenshot.is_some()
        || game
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.native_pt_capture_busy())
    {
        crate::game::offline_pt_ui::capture_busy_message(game);
        std::ops::ControlFlow::Break(())
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
