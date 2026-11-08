fn layout(sprites: &mut Sprites) -> ::core::ops::ControlFlow<()> {
    sprites.native_pt_lay_out(true);
    ::core::ops::ControlFlow::Break(())
}
