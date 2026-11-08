fn layout(sprites: &mut Sprites, pixels: bool) -> ::core::ops::ControlFlow<()> {
    sprites.native_pt_lay_out(pixels);
    ::core::ops::ControlFlow::Break(())
}
