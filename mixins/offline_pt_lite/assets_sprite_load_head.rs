fn load(
    sprites: &mut Sprites,
    root: &std::path::Path,
    name: &str,
) -> ::core::ops::ControlFlow<Result<u32>> {
    ::core::ops::ControlFlow::Break(sprites.native_pt_load(root, name))
}
