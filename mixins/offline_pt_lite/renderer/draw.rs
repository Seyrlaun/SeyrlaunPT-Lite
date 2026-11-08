fn draw(renderer: &mut Renderer, frame: &Frame) -> std::ops::ControlFlow<Result<bool>> {
    std::ops::ControlFlow::Break(renderer.native_pt_draw(frame))
}
