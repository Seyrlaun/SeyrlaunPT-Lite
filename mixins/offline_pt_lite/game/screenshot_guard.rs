fn screenshot_guard(game: &mut Game<'_>) -> std::ops::ControlFlow<()> {
    if game.ui.screenshot_message.is_some() || game.pending_screenshot.is_some()
        || game
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.native_pt_capture_busy())
    {
        game.native_chat("PT screenshot is still processing.");
        std::ops::ControlFlow::Break(())
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
