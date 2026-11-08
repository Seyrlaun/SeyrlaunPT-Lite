fn load(
    root: &std::path::Path,
    assets: &Assets,
) -> ::core::ops::ControlFlow<Result<RenderAssets>> {
    ::core::ops::ControlFlow::Break(RenderAssets::native_pt_load(root, assets))
}
