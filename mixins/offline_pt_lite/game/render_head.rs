fn render_head(game: &mut Game<'_>) -> std::ops::ControlFlow<Result<bool>> {
    std::ops::ControlFlow::Break(game.native_pt_render())
}
