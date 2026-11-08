fn offline_pt_lite_render_head(
    game: &mut crate::game::Game<'_>,
) -> std::ops::ControlFlow<Result<bool>> {
    std::ops::ControlFlow::Break(game.native_pt_render())
}
