// Shared packed program format: ctm.rs. Absolute block coordinates keep tiling
// anchored across camera-relative voxel-cache shifts and negative chunks.
fn ctm_hash(value: u32) -> u32 {
    var n=(value^(value>>16u))*0x7feb352du;
    n=(n^(n>>15u))*0x846ca68bu;
    return n^(n>>16u);
}
fn ctm_mod(n: i32, size: u32) -> u32 {
    let s=i32(size);
    return u32(((n%s)+s)%s);
}
fn ctm_plane(p: vec3<i32>, face: u32, symmetry: u32) -> vec2<i32> {
    switch face {
        case 0u: { return vec2<i32>(p.x,-p.z-1); }
        case 1u: { return p.xz; }
        case 2u: { return vec2<i32>(-p.x-1,-p.y-1); }
        case 3u: { return vec2<i32>(select(p.x,-p.x-1,symmetry==1u),-p.y-1); }
        case 4u: { return vec2<i32>(p.z,-p.y-1); }
        default: { return vec2<i32>(select(-p.z-1,p.z,symmetry==1u),-p.y-1); }
    }
}
fn ctm_horizontal(face: u32) -> vec3<i32> {
    switch face {
        case 2u: { return vec3<i32>(-1,0,0); }
        case 3u: { return vec3<i32>(1,0,0); }
        case 4u: { return vec3<i32>(0,0,1); }
        case 5u: { return vec3<i32>(0,0,-1); }
        default: { return vec3<i32>(1,0,0); }
    }
}
fn ctm_neighbor(cell: vec3<i32>, mask: u32) -> bool {
    if any(cell<vec3<i32>(0)) || any(cell>=vec3<i32>(128)) { return false; }
    let state=textureLoad(blocks,cell,0).x;
    if state>=arrayLength(&materials) { return false; }
    return (materials[state].range.w&mask)!=0u;
}
fn ctm_layer(tri: Triangle, cell: vec3<i32>) -> u32 {
    var layer=u32(tri.uv2_layer.z);
    var node=u32(tri.e2.w);
    if node==0u { return layer; }
    let base=pbr_data[3];
    let world=cell+frame.origin.xyz;
    for(var depth=0u; depth<8u; depth++) {
        if node==0u || base+node+11u>=arrayLength(&pbr_data) { break; }
        let h=base+node;
        let method=pbr_data[h];
        let face=pbr_data[h+4u];
        let symmetry=pbr_data[h+5u];
        var index=0u;
        if method==1u {
            let uv=ctm_plane(world,face,symmetry);
            index=ctm_mod(uv.y,pbr_data[h+2u])*pbr_data[h+1u]+ctm_mod(uv.x,pbr_data[h+1u]);
        } else if method==2u {
            let p=world-vec3<i32>(0,i32(pbr_data[h+7u]),0);
            var f=face;
            if symmetry==1u { f=face&0xfffffffeu; }
            if symmetry==2u { f=0u; }
            var random=u32(p.x)*73428767u ^ u32(p.y)*912931u ^ u32(p.z)*438289u ^ f*19937u;
            for(var iteration=0u; iteration<=pbr_data[h+6u]; iteration++) { random=ctm_hash(random); }
            let pick=random%pbr_data[h+9u];
            for(var tile=0u; tile<pbr_data[h+3u]; tile++) {
                if pick<pbr_data[base+pbr_data[h+10u]+tile*3u+2u] { index=tile; break; }
            }
        } else if method==3u || method==4u {
            var delta=ctm_horizontal(face);
            if method==4u { delta=vec3<i32>(0,1,0); }
            let positive=ctm_neighbor(cell+delta,pbr_data[h+8u]);
            let negative=ctm_neighbor(cell-delta,pbr_data[h+8u]);
            index=3u;
            if positive { index=0u; }
            if negative { index=2u; }
            if positive && negative { index=1u; }
            if method==4u && (index==0u || index==2u) { index=2u-index; }
        }
        let tile=base+pbr_data[h+10u]+index*3u;
        let chosen=pbr_data[tile];
        if chosen<0xfffffffeu { layer=chosen; }
        node=pbr_data[tile+1u];
    }
    return layer;
}
