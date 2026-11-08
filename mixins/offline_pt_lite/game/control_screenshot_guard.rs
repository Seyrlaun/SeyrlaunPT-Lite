fn offline_pt_lite_control_screenshot_guard(
    game: &mut crate::game::Game<'_>,
    line: &str,
) -> std::ops::ControlFlow<Result<Option<Value>>> {
    let screenshot = line.split_whitespace().next() == Some("screenshot");
    if screenshot
        && (game.ui.screenshot_message.is_some()
            || game
                .renderer
                .as_ref()
                .is_some_and(|renderer| renderer.native_pt_capture_busy()))
    {
        std::ops::ControlFlow::Break(Err("PT screenshot is still processing".into()))
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
