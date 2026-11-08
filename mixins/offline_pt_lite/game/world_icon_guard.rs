fn world_icon_guard(game: &mut Game<'_>) -> std::ops::ControlFlow<()> {
    if game.renderer.as_ref().is_some_and(|renderer| renderer.native_pt_capture_busy()) {
        std::ops::ControlFlow::Break(())
    } else {
        std::ops::ControlFlow::Continue(())
    }
}
