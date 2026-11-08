fn fingerprint(root: &std::path::Path) -> ::core::ops::ControlFlow<Option<u64>> {
    ::core::ops::ControlFlow::Break(native_pt_assets::native_pt_fingerprint(root))
}
