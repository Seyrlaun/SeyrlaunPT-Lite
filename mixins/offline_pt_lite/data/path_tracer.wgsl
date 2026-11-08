// Original terrain path tracer: section skipping + voxel DDA + local triangle intersections.
struct Frame {
    inverse: mat4x4<f32>,
    camera: vec4<f32>,
    sun: vec4<f32>,
    sky: vec4<f32>,
    image: vec4<u32>,
    options: vec4<u32>,
    optical: vec4<f32>,
    origin: vec4<i32>,
}
struct Material {
    range: vec4<u32>, tint_emission: vec4<f32>,
    surface: vec4<f32>, absorption: vec4<f32>,
}
struct Triangle {
    p0: vec4<f32>, e1: vec4<f32>, e2: vec4<f32>,
    uv0_uv1: vec4<f32>, uv2_layer: vec4<f32>,
}
struct Hit {
    t: f32, normal: vec3<f32>, albedo: vec3<f32>, emission: f32,
    state: u32, steps: u32, area: f32, front: u32,
    cell: vec3<u32>, shading_normal: vec3<f32>, surface: vec4<f32>,
}
struct Bsdf { value: vec3<f32>, pdf: f32 }
struct Medium { ior: f32, absorption: vec3<f32> }
struct Visibility { transmission: vec3<f32>, glass: bool }
struct LightConnection {
    transmission: vec3<f32>,
    incoming: vec3<f32>,
    outgoing: vec3<f32>,
    path_length: f32,
    miss: f32,
    valid: bool,
    glass: bool,
}
struct Light { cell: vec4<u32>, sampling: vec4<f32> }
@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var blocks: texture_3d<u32>;
@group(1) @binding(1) var occupancy: texture_3d<u32>;
@group(1) @binding(2) var<storage, read> materials: array<Material>;
@group(1) @binding(3) var<storage, read> triangles: array<Triangle>;
@group(1) @binding(4) var<storage, read> lights: array<Light>;
// SPRITE_BINDINGS
// CTM_CODE

@group(2) @binding(0) var previous: texture_2d<f32>;
@group(2) @binding(1) var result: texture_storage_2d<rgba32float, write>;
@group(2) @binding(2) var guides: texture_storage_2d<rgba16float, write>;

var<private> rng: u32;
// One wavelength is transported per path. The value is set once after the
// per-pixel RNG is seeded and is shared by all scalar spectral operations on
// that path.
var<private> hero_wavelength: f32;
const PI: f32 = 3.14159265359;
const EPS: f32 = 0.0002;
fn random() -> f32 {
    rng = rng * 747796405u + 2891336453u;
    let word = ((rng >> ((rng >> 28u) + 4u)) ^ rng) * 277803737u;
    return f32((word >> 22u) ^ word) * (1.0 / 4294967296.0);
}
fn basis(n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = normalize(cross(select(vec3<f32>(0.,1.,0.),vec3<f32>(1.,0.,0.),abs(n.y)>0.99),n));
    return t*v.x + cross(n,t)*v.y + n*v.z;
}
fn cosine_direction(n: vec3<f32>) -> vec3<f32> {
    let r = sqrt(random());
    let phi = 2.0*PI*random();
    return basis(n,vec3<f32>(r*cos(phi),r*sin(phi),sqrt(max(0.,1.-r*r))));
}
fn safe_direction(d: vec3<f32>) -> vec3<f32> {
    return select(select(vec3<f32>(-1e-8),vec3<f32>(1e-8),d>=vec3<f32>(0.)),d,abs(d)>vec3<f32>(1e-8));
}

fn surface_emission(mat: Material, tri: Triangle, uv: vec2<f32>, layer: u32) -> f32 {
    let header=layer*4u;
    if header+3u<arrayLength(&pbr_data) {
        let flags=pbr_data[header+2u];
        if (flags&4u)!=0u {
            let side=pbr_data[header+1u];
            let safe_uv=clamp(uv,vec2<f32>(0.),vec2<f32>(0.999999));
            let xy=vec2<u32>(safe_uv*f32(side));
            let index=xy.y*side+xy.x;
            let word=pbr_data[pbr_data[header]+side*side+index/4u];
            let value=(word>>((index%4u)*8u))&255u;
            // Shared with EMISSION_POWER, supplied in the frame uniform.
            return frame.optical.z*f32(value)/254.;
        }
    }
    let strength=select(mat.tint_emission.w,tri.e1.w,mat.range.z>0u);
    switch u32(tri.uv2_layer.w) {
        // Vanilla torch UVs put the ember head in rows 6 and 7.
        case 1u: { return strength*select(0.,1.,uv.y<0.5); }
        // Lantern window, excluding its base, cap and handle.
        case 2u: {
            let window=uv.x>1./16. && uv.x<5./16. && uv.y>3./16. && uv.y<8./16.;
            return strength*select(0.,1.,window);
        }
        default: { return strength; }
    }
}

// labPBR's normal Y points down, along increasing texture V. Derivatives
// preserve this orientation for rotated/mirrored UVs and back-facing quads.
fn mapped_normal(tri: Triangle, n: vec3<f32>, xy: vec2<f32>, rd: vec3<f32>) -> vec3<f32> {
    let a=tri.uv0_uv1.zw-tri.uv0_uv1.xy;
    let b=tri.uv2_layer.xy-tri.uv0_uv1.xy;
    let det=a.x*b.y-a.y*b.x;
    if abs(det)<1e-8 { return n; }
    let du=(tri.e1.xyz*b.y-tri.e2.xyz*a.y)/det;
    let dv=(tri.e2.xyz*a.x-tri.e1.xyz*b.x)/det;
    let tangent=du-n*dot(n,du);
    if dot(tangent,tangent)<1e-12 { return n; }
    let t=normalize(tangent);
    let bitangent=cross(n,t)*select(-1.,1.,dot(cross(n,t),dv)>=0.);
    let z=sqrt(max(0.,1.-dot(xy,xy)));
    let mapped=normalize(t*xy.x+bitangent*xy.y+n*max(z,0.001));
    // Keep the shading frame facing the incident ray.
    return select(n,mapped,dot(mapped,-rd)>0.001 && dot(mapped,n)>0.001);
}

fn apply_pbr(hit: Hit, tri: Triangle, uv: vec2<f32>, rd: vec3<f32>, mat: Material, layer: u32) -> Hit {
    var out=hit;
    out.shading_normal=hit.normal;
    // roughness, metalness, dielectric F0, optional conductor code
    out.surface=vec4<f32>(mat.surface.xy,0.04,255.);
    // Keep smooth dielectric boundary normals for the current glass transport.
    if mat.surface.z>0.5 { return out; }
    let header=layer*4u;
    if header+3u>=arrayLength(&pbr_data) { return out; }
    let flags=pbr_data[header+2u];
    if flags==0u { return out; }
    let side=pbr_data[header+1u];
    let safe_uv=clamp(uv,vec2<f32>(0.),vec2<f32>(0.999999));
    let xy=vec2<u32>(safe_uv*f32(side));
    let word=pbr_data[pbr_data[header]+xy.y*side+xy.x];
    if (flags&1u)!=0u {
        let normal=vec2<f32>(f32(word&255u),f32((word>>8u)&255u))/255.*2.-1.;
        out.shading_normal=mapped_normal(tri,hit.normal,normal,rd);
    }
    if (flags&2u)!=0u {
        let smoothness=f32((word>>16u)&255u)/255.;
        let reflectance=(word>>24u)&255u;
        // eval/sample_bsdf already square perceptual roughness into GGX alpha.
        out.surface=vec4<f32>(1.-smoothness,select(0.,1.,reflectance>=230u),
            f32(reflectance)/255.,f32(reflectance));
    }
    return out;
}

fn trace_segment(ro: vec3<f32>, direction: vec3<f32>, limit: f32, previous_state: u32) -> Hit {
    var hit: Hit;
    hit.t = -1.0;
    let rd = safe_direction(direction);
    let inv = 1.0/rd;
    let ta = (vec3<f32>(0.)-ro)*inv;
    let tb = (vec3<f32>(128.)-ro)*inv;
    let lo = min(ta,tb);
    let hi = max(ta,tb);
    var t = max(0.,max(lo.x,max(lo.y,lo.z)))+EPS;
    let end = min(limit,min(hi.x,min(hi.y,hi.z)));
    for (var step=0u;step<768u;step++) {
        if t>=end { break; }
        hit.steps = step;
        let p = ro + rd*t;
        let cell = vec3<i32>(floor(p));
        if any(cell<vec3<i32>(0)) || any(cell>=vec3<i32>(128)) { break; }
        let brick = cell/16;
        let empty = textureLoad(occupancy,brick,0).x==0u;
        let extent = select(1.,16.,empty);
        let cell_base = floor(p/extent)*extent;
        let boundary = cell_base + select(vec3<f32>(0.),vec3<f32>(extent),rd>vec3<f32>(0.));
        let next_axis = (boundary-ro)*inv;
        let next_t = min(next_axis.x,min(next_axis.y,next_axis.z));
        if !empty {
            let state = textureLoad(blocks,cell,0).x;
            if state<arrayLength(&materials) {
                let mat = materials[state];
                var best = min(next_t+EPS,end);
                for (var j=0u;j<mat.range.y;j++) {
                    let tri = triangles[mat.range.x+j];
                    let h = cross(rd,tri.e2.xyz);
                    let det = dot(tri.e1.xyz,h);
                    if abs(det)<1e-8 { continue; }
                    let s = ro-vec3<f32>(cell)-tri.p0.xyz;
                    let u = dot(s,h)/det;
                    let q = cross(s,tri.e1.xyz);
                    let v = dot(rd,q)/det;
                    let distance = dot(tri.e2.xyz,q)/det;
                    // A continuation offset can cross an opaque surface
                    // touching the preceding dielectric. Recover only that
                    // new front face; never re-hit the originating interface.
                    let contact=previous_state!=0xffffffffu && state!=previous_state &&
                        materials[previous_state].surface.z>0.5 &&
                        mat.surface.z<0.5 && t<EPS*2. && distance>=-EPS*8. &&
                        dot(cross(tri.e1.xyz,tri.e2.xyz),rd)<0.;
                    if u<0. || v<0. || u+v>1. || distance>best { continue; }
                    if !contact && (distance<EPS || distance<t-EPS*2.) { continue; }
                    let uv = tri.uv0_uv1.xy*(1.-u-v)+tri.uv0_uv1.zw*u+tri.uv2_layer.xy*v;
                    let ng = cross(tri.e1.xyz,tri.e2.xyz);
                    var n=normalize(ng);
                    if mat.surface.z>0.5 && mat.absorption.w<0.5 {
                        // Treat adjacent identical glass as one optical volume,
                        // but only when both samples around the actual face are
                        // the same state. This avoids hiding a real outer face
                        // when a model winding or oblique ray is unusual.
                        let point=ro+rd*distance;
                        let side_a=vec3<i32>(floor(point+ng*EPS*8.));
                        let side_b=vec3<i32>(floor(point-ng*EPS*8.));
                        if all(side_a>=vec3<i32>(0)) && all(side_a<vec3<i32>(128)) &&
                            all(side_b>=vec3<i32>(0)) && all(side_b<vec3<i32>(128)) {
                            let state_a=textureLoad(blocks,side_a,0).x;
                            let state_b=textureLoad(blocks,side_b,0).x;
                            if state_a==state && state_b==state { continue; }
                        }
                    }
                    let layer = ctm_layer(tri,cell);
                    let color = texel(layer,uv);
                    // The transparent centers of vanilla glass textures still define
                    // a dielectric interface; alpha testing applies to cutouts only.
                    if color.a<0.1 && mat.surface.z<0.5 { continue; }
                    let emission=surface_emission(mat,tri,uv,layer);
                    // Emissive cutout overlays can share the foliage plane.
                    if hit.t>0. && abs(distance-best)<1e-6 && hit.emission>emission { continue; }
                    best=distance;
                    hit.t=max(0.,distance);
                    hit.front=select(0u,1u,dot(n,rd)<0.);
                    n=select(n,-n,dot(n,rd)>0.);
                    hit.normal=n;
                    hit.albedo=max(vec3<f32>(0.001),color.rgb*mix(vec3<f32>(1.),mat.tint_emission.rgb,tri.p0.w));
                    hit.emission=emission;
                    hit.state=state;
                    hit.cell=vec3<u32>(cell);
                    hit.area=length(ng)*0.5;
                    hit=apply_pbr(hit,tri,uv,rd,mat,layer);
                }
                if hit.t>=0. { return hit; }
            }
        }
        t=max(t+EPS,next_t+EPS);
    }
    return hit;
}
fn trace(ro: vec3<f32>, direction: vec3<f32>, limit: f32) -> Hit {
    return trace_segment(ro,direction,limit,0xffffffffu);
}

fn environment(d: vec3<f32>) -> vec3<f32> {
    return frame.sky.rgb * mix(0.35,1.3,clamp(d.y*0.5+0.5,0.,1.));
}

fn power_weight(a: f32, b: f32) -> f32 {
    return a*a/max(a*a+b*b,1e-20);
}
fn sun_pdf() -> f32 {
    return 1./max(2.*PI*(1.-cos(frame.optical.y)),1e-8);
}
fn sun_radiance() -> vec3<f32> {
    return vec3<f32>(1.,0.90,0.75)*frame.sun.w*sun_pdf();
}
fn fresnel_schlick(f0: vec3<f32>, cosine: f32) -> vec3<f32> {
    return f0+(vec3<f32>(1.)-f0)*pow(clamp(1.-cosine,0.,1.),5.);
}
fn smith_g1(cosine: f32, a2: f32) -> f32 {
    return 2.*cosine/max(cosine+sqrt(a2+(1.-a2)*cosine*cosine),1e-7);
}
fn specular_probability(surface: vec4<f32>) -> f32 {
    return mix(clamp(surface.z,0.2,0.9),1.,surface.y);
}
fn conductor_f0(code: u32, albedo: vec3<f32>) -> vec3<f32> {
    var n: vec3<f32>;
    var k: vec3<f32>;
    switch code {
        case 230u: { n=vec3<f32>(2.9114,2.9497,2.5845); k=vec3<f32>(3.0893,2.9318,2.7670); }
        case 231u: { n=vec3<f32>(0.18299,0.42108,1.3734); k=vec3<f32>(3.4242,2.3459,1.7704); }
        case 232u: { n=vec3<f32>(1.3456,0.96521,0.61722); k=vec3<f32>(7.4746,6.3995,5.3031); }
        case 233u: { n=vec3<f32>(3.1071,3.1812,2.3230); k=vec3<f32>(3.3314,3.3291,3.1350); }
        case 234u: { n=vec3<f32>(0.27105,0.67693,1.3164); k=vec3<f32>(3.6092,2.6248,2.2921); }
        case 235u: { n=vec3<f32>(1.91,1.83,1.44); k=vec3<f32>(3.51,3.40,3.18); }
        case 236u: { n=vec3<f32>(2.3757,2.0847,1.8453); k=vec3<f32>(4.2655,3.7153,3.1365); }
        case 237u: { n=vec3<f32>(0.15943,0.14512,0.13547); k=vec3<f32>(3.9291,3.1900,2.3808); }
        default: { return clamp(albedo,vec3<f32>(0.001),vec3<f32>(0.98)); }
    }
    return ((n-1.)*(n-1.)+k*k)/((n+1.)*(n+1.)+k*k)*albedo;
}
fn eval_bsdf(wo: vec3<f32>, wi: vec3<f32>, n: vec3<f32>, albedo: vec3<f32>, surface: vec4<f32>) -> Bsdf {
    var out: Bsdf;
    let ni=max(0.,dot(n,wi));
    let no=max(0.,dot(n,wo));
    if ni<=0. || no<=0. { return out; }
    let h=normalize(wo+wi);
    let nh=max(0.,dot(n,h));
    let vh=max(0.,dot(wo,h));
    let alpha=max(0.001,surface.x*surface.x);
    let a2=alpha*alpha;
    // Cross product avoids cancellation at nearly perfect mirror highlights.
    let nh_cross=cross(n,h);
    let denominator=dot(nh_cross,nh_cross)+a2*nh*nh;
    let distribution=a2/(PI*denominator*denominator);
    let f0=mix(vec3<f32>(surface.z),conductor_f0(u32(surface.w),albedo),surface.y);
    let fresnel=fresnel_schlick(f0,vh);
    let geometry=smith_g1(ni,a2)*smith_g1(no,a2);
    let diffuse=(1.-surface.y)*albedo/PI*(vec3<f32>(1.)-fresnel);
    out.value=diffuse+distribution*geometry*fresnel/max(4.*ni*no,1e-7);
    // Visible-normal GGX: the half-vector Jacobian cancels wo.h.
    let specular_pdf=distribution*smith_g1(no,a2)/max(4.*no,1e-7);
    out.pdf=mix(ni/PI,specular_pdf,specular_probability(surface));
    return out;
}
fn sample_bsdf(wo: vec3<f32>, n: vec3<f32>, surface: vec4<f32>) -> vec3<f32> {
    if random()<specular_probability(surface) {
        let alpha=max(0.001,surface.x*surface.x);
        let t=normalize(cross(select(vec3<f32>(0.,1.,0.),vec3<f32>(1.,0.,0.),abs(n.y)>0.99),n));
        let b=cross(n,t);
        let v=normalize(vec3<f32>(alpha*dot(wo,t),alpha*dot(wo,b),dot(wo,n)));
        let lensq=dot(v.xy,v.xy);
        let t1=select(vec3<f32>(1.,0.,0.),vec3<f32>(-v.y,v.x,0.)/sqrt(max(lensq,1e-20)),lensq>0.);
        let t2=cross(v,t1);
        let r=sqrt(random());
        let phi=2.*PI*random();
        let x=r*cos(phi);
        let s=0.5*(1.+v.z);
        let y=mix(sqrt(max(0.,1.-x*x)),r*sin(phi),s);
        let nh=t1*x+t2*y+v*sqrt(max(0.,1.-x*x-y*y));
        let local=normalize(vec3<f32>(alpha*nh.xy,max(0.,nh.z)));
        let h=t*local.x+b*local.y+n*local.z;
        return reflect(-wo,h);
    }
    return cosine_direction(n);
}
fn fresnel_dielectric(ci: f32, eta_i: f32, eta_t: f32) -> f32 {
    let sin_t2=(eta_i/eta_t)*(eta_i/eta_t)*max(0.,1.-ci*ci);
    if sin_t2>=1. { return 1.; }
    let ct=sqrt(1.-sin_t2);
    let parallel=(eta_t*ci-eta_i*ct)/(eta_t*ci+eta_i*ct);
    let perpendicular=(eta_i*ci-eta_t*ct)/(eta_i*ci+eta_t*ct);
    return 0.5*(parallel*parallel+perpendicular*perpendicular);
}
fn sample_light() -> Light {
    let threshold=min(random(),0.99999994);
    var lo=0u;
    var hi=frame.options.y-1u;
    while lo<hi {
        let mid=(lo+hi)/2u;
        if threshold<lights[mid].sampling.x { hi=mid; } else { lo=mid+1u; }
    }
    return lights[lo];
}
fn light_probability(cell: vec3<u32>) -> f32 {
    var lo=0u;
    var hi=frame.options.y;
    while lo<hi {
        let mid=(lo+hi)/2u;
        let at=lights[mid].cell.xyz;
        let less=at.x<cell.x || (at.x==cell.x && (at.y<cell.y || (at.y==cell.y && at.z<cell.z)));
        if less { lo=mid+1u; } else { hi=mid; }
    }
    if lo<frame.options.y {
        if all(lights[lo].cell.xyz==cell) { return lights[lo].sampling.y; }
    }
    return 0.;
}
fn medium_from_state(state: u32) -> Medium {
    if state<arrayLength(&materials) {
        let mat=materials[state];
        if mat.surface.z>0.5 && mat.absorption.w<0.5 {
            return Medium(mat.surface.w,mat.absorption.rgb);
        }
    }
    return Medium(1.,vec3<f32>(0.));
}
fn medium_at(p: vec3<f32>) -> Medium {
    let cell=vec3<i32>(floor(p));
    if all(cell>=vec3<i32>(0)) && all(cell<vec3<i32>(128)) {
        return medium_from_state(textureLoad(blocks,cell,0).x);
    }
    return Medium(1.,vec3<f32>(0.));
}
fn interface_media(point: vec3<f32>, hit: Hit, current: Medium) -> vec2<f32> {
    // The ray carries the incident medium explicitly. Voxel probes are only
    // used for the transmitted side, so a grazing hit cannot silently turn an
    // exit into air->air and send the ray into the wrong block behind glass.
    let transmitted=medium_at(point-hit.normal*EPS*8.);
    let surface=materials[hit.state];
    var eta_i=max(current.ior,1.);
    var eta_t=max(transmitted.ior,1.);
    if hit.front>0u && eta_t<=1.001 {
        eta_t=max(surface.surface.w,1.);
    }
    if hit.front==0u && eta_i<=1.001 {
        eta_i=max(surface.surface.w,1.);
    }
    // At a coplanar contact both probes can resolve to the same voxel. The
    // declared surface IOR is the only stable fallback in that case.
    if abs(eta_i-eta_t)<1e-4 {
        if hit.front>0u { eta_t=max(surface.surface.w,1.); }
        else { eta_i=max(surface.surface.w,1.); }
    }
    return vec2<f32>(eta_i,eta_t);
}
// Straight shadow connections approximate solid-glass transmission; camera
// paths below still refract. This does not solve focused refractive caustics.
fn shadow_transmittance(origin: vec3<f32>, d: vec3<f32>, limit: f32) -> Visibility {
    var p=origin;
    var remaining=limit;
    var transmission=vec3<f32>(1.);
    var medium=medium_at(origin+d*EPS*4.);
    var glass=false;
    var previous_state=0xffffffffu;
    for (var step=0u;step<32u;step++) {
        let hit=trace_segment(p,d,remaining,previous_state);
        if hit.t<0. { return Visibility(transmission*exp(-medium.absorption*remaining),glass); }
        transmission *= exp(-medium.absorption*hit.t);
        let mat=materials[hit.state];
        if mat.surface.z<0.5 { return Visibility(vec3<f32>(0.),glass); }
        glass=true;
        previous_state=hit.state;
        p += d*hit.t;
        let ci=clamp(dot(-d,hit.normal),0.,1.);
        if mat.absorption.w>0.5 {
            // A closed thin pane's front and back represent one thin sheet.
            if hit.front>0u {
                let f=fresnel_dielectric(ci,1.,mat.surface.w);
                transmission *= (1.-2.*f/(1.+f))*exp(-mat.absorption.rgb*0.0625);
            }
        } else {
            let media=interface_media(p,hit,medium);
            transmission *= 1.-fresnel_dielectric(ci,media.x,media.y);
            medium=medium_at(p+d*EPS*8.);
        }
        if max(transmission.r,max(transmission.g,transmission.b))<1e-5 {
            return Visibility(vec3<f32>(0.),glass);
        }
        let advance=hit.t+EPS*4.;
        remaining -= advance;
        if remaining<=0. { return Visibility(transmission,glass); }
        p += d*EPS*4.;
    }
    return Visibility(vec3<f32>(0.),glass);
}
// Follow a sampled emitter connection through solid glass. The straight
// visibility test above is still used as a conservative fallback, but this
// path solves the direction at the light-facing endpoint so a refracted ray
// does not illuminate the wrong screen-space point.
fn refracted_light_connection(origin: vec3<f32>, light_point: vec3<f32>) -> LightConnection {
    let fallback=normalize(light_point-origin);
    var start=fallback;
    var result=LightConnection(vec3<f32>(0.),fallback,fallback,
        distance(light_point,origin),1e9,false,false);
    for (var iteration=0u;iteration<4u;iteration++) {
        var p=origin;
        var d=start;
        var medium=medium_at(p+d*EPS*4.);
        var transmission=vec3<f32>(1.);
        var path_length=0.;
        var remaining=512.;
        var previous_state=0xffffffffu;
        var miss=vec3<f32>(0.);
        var endpoint_d=d;
        var endpoint_length=0.;
        var saw_glass=false;
        var reached=false;
        for (var step=0u;step<32u;step++) {
            let to_target=light_point-p;
            let along=dot(to_target,d);
            if along<=EPS { break; }
            let hit=trace_segment(p,d,min(remaining,along+EPS*8.),previous_state);
            if hit.t<0. || hit.t>along+EPS*4. {
                let endpoint=p+d*along;
                miss=endpoint-light_point;
                endpoint_d=d;
                endpoint_length=path_length+along;
                transmission *= exp(-medium.absorption*along);
                reached=true;
                break;
            }
            transmission *= exp(-medium.absorption*hit.t);
            let mat=materials[hit.state];
            if mat.surface.z<0.5 { break; }
            saw_glass=true;
            p += d*hit.t;
            let ci=clamp(dot(-d,hit.normal),0.,1.);
            if mat.absorption.w>0.5 {
                if hit.front>0u {
                    let f=fresnel_dielectric(ci,1.,mat.surface.w);
                    transmission *= (1.-2.*f/(1.+f))*exp(-mat.absorption.rgb*0.0625);
                }
            } else {
                let media=interface_media(p,hit,medium);
                let f=fresnel_dielectric(ci,media.x,media.y);
                transmission *= 1.-f;
                let eta=media.x/max(media.y,1e-6);
                let refracted=refract(d,hit.normal,eta);
                if length(refracted)<1e-4 { break; }
                d=normalize(refracted);
                medium=medium_at(p+d*EPS*8.);
            }
            let advance=hit.t+EPS*4.;
            path_length += advance;
            remaining -= advance;
            if remaining<=0. { break; }
            p += d*EPS*4.;
            previous_state=hit.state;
        }
        if reached {
            let miss_length=length(miss);
            result=LightConnection(transmission,start,endpoint_d,endpoint_length,
                miss_length,miss_length<0.035,saw_glass);
            if result.valid { return result; }
            if miss_length<1e-5 { return result; }
            // Correct the launch direction in the plane perpendicular to the
            // current ray. This converges quickly for the planar voxel faces
            // used by panes and solid glass blocks.
            let correction=miss-(start*dot(miss,start));
            start=normalize(start-correction/max(endpoint_length,1.));
        } else {
            break;
        }
    }
    return result;
}
fn direct_light(p: vec3<f32>, ng: vec3<f32>, n: vec3<f32>, wo: vec3<f32>, albedo: vec3<f32>, surface: vec4<f32>, continuation: bool) -> vec3<f32> {
    var radiance=vec3<f32>(0.);
    // Sky is direct illumination too; it must not require another surface bounce.
    let sky_direction=cosine_direction(n);
    if dot(ng,sky_direction)>0. {
        let visibility=shadow_transmittance(p+ng*EPS*4.,sky_direction,512.);
        let bsdf=eval_bsdf(wo,sky_direction,n,albedo,surface);
        let pdf=max(0.,dot(n,sky_direction))/PI;
        let weight=select(1.,power_weight(pdf,bsdf.pdf),continuation && !visibility.glass);
        radiance += visibility.transmission*bsdf.value*environment(sky_direction)*PI*weight;
    }
    if frame.sun.w>0. {
        let phi=random()*2.*PI;
        let ct=mix(cos(frame.optical.y),1.,random());
        let st=sqrt(max(0.,1.-ct*ct));
        let d=basis(frame.sun.xyz,vec3<f32>(st*cos(phi),st*sin(phi),ct));
        let cosine=max(0.,dot(n,d));
        if cosine>0. && dot(ng,d)>0. {
            let visibility=shadow_transmittance(p+ng*EPS*4.,d,512.);
            let bsdf=eval_bsdf(wo,d,n,albedo,surface);
            let weight=select(1.,power_weight(sun_pdf(),bsdf.pdf),continuation && !visibility.glass);
            radiance += visibility.transmission*bsdf.value*vec3<f32>(1.,0.90,0.75)*frame.sun.w*cosine*weight;
        }
    }
    if frame.options.y>0u {
        let light=sample_light();
        let light_material=materials[light.cell.w];
        let tri=triangles[light_material.range.x+min(u32(random()*f32(light_material.range.y)),light_material.range.y-1u)];
        let u=sqrt(random());
        let v=random();
        let b=vec2<f32>(u*(1.-v),u*v);
        let light_position=vec3<f32>(light.cell.xyz)+tri.p0.xyz+tri.e1.xyz*b.x+tri.e2.xyz*b.y;
        let delta=light_position-p;
        let distance=length(delta);
        let d=delta/max(distance,EPS);
        let light_ng=cross(tri.e1.xyz,tri.e2.xyz);
        let area=length(light_ng)*0.5;
        let cos_l=abs(dot(normalize(light_ng),-d));
        let cos_s=max(0.,dot(n,d));
        if cos_s>0. && dot(ng,d)>0. && cos_l>0. && distance>EPS*8. {
            let visibility=shadow_transmittance(p+ng*EPS*4.,d,distance-EPS*8.);
            var connection=LightConnection(visibility.transmission,d,-d,distance,
                0.,false,visibility.glass);
            if visibility.glass {
                connection=refracted_light_connection(p+ng*EPS*4.,light_position);
                if !connection.valid {
                    connection=LightConnection(visibility.transmission,d,-d,distance,
                        0.,true,visibility.glass);
                }
            }
            let connection_d=connection.incoming;
            let connection_cos_s=max(0.,dot(n,connection_d));
            let connection_cos_ng=max(0.,dot(ng,connection_d));
            let connection_cos_l=abs(dot(normalize(light_ng),connection.outgoing));
            if any(connection.transmission>vec3<f32>(0.)) &&
                connection_cos_s>0. && connection_cos_ng>0. &&
                connection_cos_l>0. {
                let uv=tri.uv0_uv1.xy*(1.-b.x-b.y)+tri.uv0_uv1.zw*b.x+tri.uv2_layer.xy*b.y;
                let layer=ctm_layer(tri,vec3<i32>(light.cell.xyz));
                let color=texel(layer,uv);
                let le=color.rgb*mix(vec3<f32>(1.),light_material.tint_emission.rgb,tri.p0.w)*surface_emission(light_material,tri,uv,layer);
                if color.a>=0.1 {
                    let connection_distance=max(connection.path_length,EPS);
                    let pdf=connection_distance*connection_distance*light.sampling.y/max(connection_cos_l*area*f32(light_material.range.y),1e-12);
                    let bsdf=eval_bsdf(wo,connection_d,n,albedo,surface);
                    let weight=select(1.,power_weight(pdf,bsdf.pdf),continuation && !connection.glass);
                    radiance += connection.transmission*bsdf.value*le*connection_cos_s/max(pdf,1e-12)*weight;
                }
            }
        }
    }
    return radiance;
}

fn primary_direction(pixel: vec2<f32>) -> vec3<f32> {
    let uv=pixel/vec2<f32>(frame.image.xy);
    let clip=frame.inverse*vec4<f32>(uv.x*2.-1.,1.-uv.y*2.,1.,1.);
    return normalize(clip.xyz/clip.w);
}

// Spectral transport uses one continuously jittered hero wavelength per path.
// The four strata are only a low-discrepancy schedule over the visible
// interval; they are not four fake RGB transport channels. Existing resource
// packs still provide RGB texels, so the scalar path reconstructs a bounded
// reflectance at the input boundary. A future measured spectral pack can
// replace that boundary without changing the path integrator.
fn spectral_gaussian(lambda: f32, center: f32, width: f32) -> f32 {
    let d=(lambda-center)/width;
    return exp(-0.5*d*d);
}
fn cie_xyz_hero(lambda: f32) -> vec3<f32> {
    // Smooth analytic fit to the CIE 1931 2-degree observer. This avoids
    // embedding a large external table in the shader while retaining a
    // continuous observer response for jittered hero wavelengths.
    let x=1.056*spectral_gaussian(lambda,599.8,37.9)+
        0.362*spectral_gaussian(lambda,442.0,16.0)-
        0.065*spectral_gaussian(lambda,501.1,20.4);
    let y=0.821*spectral_gaussian(lambda,568.8,46.9)+
        0.286*spectral_gaussian(lambda,530.9,16.3);
    let z=1.217*spectral_gaussian(lambda,437.0,11.8)+
        0.681*spectral_gaussian(lambda,459.0,26.0);
    return max(vec3<f32>(x,y,z),vec3<f32>(0.));
}
fn spectral_hero_to_rgb(radiance: f32) -> vec3<f32> {
    // Uniform sampling over 360..830 nm. The estimator multiplies by the
    // visible interval (1 / pdf), then normalizes each output channel by the
    // response of an equal-energy spectrum. A shared denominator would turn
    // neutral grey into a colour cast because the compact analytic observer
    // fit is not exactly energy-normalized in XYZ.
    let xyz=cie_xyz_hero(hero_wavelength)*max(radiance,0.)*470.;
    let linear=vec3<f32>(
        3.2406*xyz.x-1.5372*xyz.y-0.4986*xyz.z,
        -0.9689*xyz.x+1.8758*xyz.y+0.0415*xyz.z,
        0.0557*xyz.x-0.2040*xyz.y+1.0570*xyz.z);
    // Numerical integrals of cie_xyz_hero over 360..830 nm after the same
    // XYZ-to-linear-sRGB matrix. Keep these explicit so the shader does not
    // need a large observer table or a runtime integration loop.
    let equal_energy_white=vec3<f32>(157.53905,97.48715,69.13958);
    return linear/equal_energy_white;
}
fn spectral_conductor_f0_value(code: u32, albedo: vec3<f32>, band: u32) -> f32 {
    // The authored labPBR conductor table has three representative channels.
    // Interpolate those normal-incidence values over the continuous hero
    // wavelength instead of indexing a four-band approximation.
    let f0=conductor_f0(code,albedo);
    return spectral_interp4(spectral_lambda(band),f0.b,f0.g,f0.r,f0.r);
}

// The spectral integrator samples one wavelength per path. This is the
// standard hero-wavelength formulation: every path has one scalar transport
// value and one wavelength-dependent direction, so glass dispersion changes
// the actual ray instead of merely tinting a shared RGB ray. Four strata are
// scheduled over successive accumulation samples and converted to XYZ only
// after transport.
fn spectral_band(id: vec3<u32>) -> u32 {
    return (frame.image.w+id.x+id.y*3u)&3u;
}
fn spectral_lambda(band: u32) -> f32 {
    // Scalar transport uses the continuously jittered hero wavelength. The
    // argument remains part of the helper's contract for call-site clarity.
    return hero_wavelength;
}
fn spectral_value(color: vec3<f32>, band: u32) -> f32 {
    let lambda=spectral_lambda(band);
    let white=min(color.r,min(color.g,color.b));
    let r=max(color.r-white,0.);
    let g=max(color.g-white,0.);
    let b=max(color.b-white,0.);
    // Smooth positive basis functions are used at the RGB texture boundary.
    // They are a bounded reconstruction, not a claim that an RGB pack contains
    // measured spectral reflectance.
    let red=exp(-0.5*pow((lambda-610.)/55.,2.));
    let green=exp(-0.5*pow((lambda-540.)/38.,2.));
    let blue=exp(-0.5*pow((lambda-450.)/28.,2.));
    return max(0.,white+r*red+g*green+b*blue);
}
fn spectral_absorption_value(color: vec3<f32>, band: u32) -> f32 {
    let lambda=spectral_lambda(band);
    // Interpolate the authored RGB extinction coefficients over wavelength.
    // This keeps Beer attenuation continuous while retaining the pack's
    // three-channel material input boundary.
    return max(0.,spectral_interp4(
        lambda,color.b,color.g,color.r,color.r*0.85));
}
fn spectral_interp4(lambda: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    if lambda<=360. { return 0.; }
    if lambda<450. { return mix(0.,a,smoothstep(360.,450.,lambda)); }
    if lambda<530. { return mix(a,b,smoothstep(450.,530.,lambda)); }
    if lambda<610. { return mix(b,c,smoothstep(530.,610.,lambda)); }
    if lambda<690. { return mix(c,d,smoothstep(610.,690.,lambda)); }
    return mix(d,0.,smoothstep(690.,830.,lambda));
}
fn spectral_ior(base: f32, band: u32) -> f32 {
    let lambda=spectral_lambda(band);
    // Air/vacuum is nondispersive. Applying a Cauchy offset to eta=1 was the
    // source of an artificial air/glass index mismatch in the first prototype.
    if base<=1.00001 { return 1.; }
    // A compact relative Cauchy term. Dispersion scales with the medium's
    // refractivity, so water and glass remain plausible at different IORs.
    return max(1.,base+(base-1.)*0.02*((550./lambda)*(550./lambda)-1.));
}
fn spectral_fresnel_value(ci: f32, eta_i: f32, eta_t: f32, band: u32) -> f32 {
    return fresnel_dielectric(ci,spectral_ior(eta_i,band),spectral_ior(eta_t,band));
}
struct SpectralBsdfScalar { value: f32, pdf: f32 }
fn eval_bsdf_spectral_scalar(
    wo: vec3<f32>,
    wi: vec3<f32>,
    n: vec3<f32>,
    albedo: vec3<f32>,
    surface: vec4<f32>,
    band: u32,
) -> SpectralBsdfScalar {
    var out=SpectralBsdfScalar(0.,0.);
    let ni=max(0.,dot(n,wi));
    let no=max(0.,dot(n,wo));
    if ni<=0. || no<=0. { return out; }
    let h=normalize(wo+wi);
    let nh=max(0.,dot(n,h));
    let vh=max(0.,dot(wo,h));
    let alpha=max(0.001,surface.x*surface.x);
    let a2=alpha*alpha;
    let nh_cross=cross(n,h);
    let denominator=dot(nh_cross,nh_cross)+a2*nh*nh;
    let distribution=a2/(PI*denominator*denominator);
    let conductor=spectral_conductor_f0_value(u32(surface.w),albedo,band);
    let f0=mix(surface.z,conductor,surface.y);
    let fresnel=f0+(1.-f0)*pow(clamp(1.-vh,0.,1.),5.);
    let geometry=smith_g1(ni,a2)*smith_g1(no,a2);
    let diffuse=(1.-fresnel)*spectral_value(albedo,band)/PI*(1.-surface.y);
    out.value=diffuse+distribution*geometry*fresnel/max(4.*ni*no,1e-7);
    let specular_pdf=distribution*smith_g1(no,a2)/max(4.*no,1e-7);
    out.pdf=mix(ni/PI,specular_pdf,specular_probability(surface));
    return out;
}
struct SpectralMediumScalar { ior: f32, absorption: f32 }
fn spectral_medium_at_scalar(p: vec3<f32>, band: u32) -> SpectralMediumScalar {
    let cell=vec3<i32>(floor(p));
    if all(cell>=vec3<i32>(0)) && all(cell<vec3<i32>(128)) {
        let state=textureLoad(blocks,cell,0).x;
        if state<arrayLength(&materials) {
            let mat=materials[state];
            if mat.surface.z>0.5 && mat.absorption.w<0.5 {
                return SpectralMediumScalar(
                    mat.surface.w,spectral_absorption_value(mat.absorption.rgb,band));
            }
        }
    }
    return SpectralMediumScalar(1.,0.);
}
struct SpectralVisibilityScalar { transmission: f32, glass: bool }
fn shadow_transmittance_spectral_scalar(
    origin: vec3<f32>, d: vec3<f32>, limit: f32, band: u32,
) -> SpectralVisibilityScalar {
    var p=origin;
    var remaining=limit;
    var transmission=1.;
    var medium=spectral_medium_at_scalar(origin+d*EPS*4.,band);
    var glass=false;
    var previous_state=0xffffffffu;
    for (var step=0u;step<32u;step++) {
        let hit=trace_segment(p,d,remaining,previous_state);
        if hit.t<0. { return SpectralVisibilityScalar(
            transmission*exp(-medium.absorption*remaining),glass); }
        transmission*=exp(-medium.absorption*hit.t);
        let mat=materials[hit.state];
        if mat.surface.z<0.5 { return SpectralVisibilityScalar(0.,glass); }
        glass=true;
        previous_state=hit.state;
        p+=d*hit.t;
        let ci=clamp(dot(-d,hit.normal),0.,1.);
        if mat.absorption.w>0.5 {
            if hit.front>0u {
                let f=spectral_fresnel_value(ci,1.,mat.surface.w,band);
                transmission*=(1.-2.*f/(1.+f))*
                    exp(-spectral_absorption_value(mat.absorption.rgb,band)*0.0625);
            }
        } else {
            let media=interface_media(p,hit,Medium(medium.ior,vec3<f32>(medium.absorption)));
            let f=spectral_fresnel_value(ci,media.x,media.y,band);
            transmission*=1.-f;
            medium=spectral_medium_at_scalar(p+d*EPS*8.,band);
        }
        if transmission<1e-5 { return SpectralVisibilityScalar(0.,glass); }
        let advance=hit.t+EPS*4.;
        remaining-=advance;
        if remaining<=0. { return SpectralVisibilityScalar(transmission,glass); }
        p+=d*EPS*4.;
    }
    return SpectralVisibilityScalar(0.,glass);
}
fn spectral_environment_value(d: vec3<f32>, band: u32) -> f32 {
    return spectral_value(frame.sky.rgb,band)*mix(0.35,1.3,clamp(d.y*0.5+0.5,0.,1.));
}
fn spectral_sun_value(band: u32) -> f32 {
    return spectral_value(vec3<f32>(1.,0.90,0.75),band)*frame.sun.w*sun_pdf();
}
fn direct_light_spectral_scalar(
    p: vec3<f32>,
    ng: vec3<f32>,
    n: vec3<f32>,
    wo: vec3<f32>,
    albedo: vec3<f32>,
    surface: vec4<f32>,
    continuation: bool,
    band: u32,
) -> f32 {
    var radiance=0.;
    let sky_direction=cosine_direction(n);
    if dot(ng,sky_direction)>0. {
        let visibility=shadow_transmittance_spectral_scalar(
            p+ng*EPS*4.,sky_direction,512.,band);
        let bsdf=eval_bsdf_spectral_scalar(wo,sky_direction,n,albedo,surface,band);
        let pdf=max(0.,dot(n,sky_direction))/PI;
        let weight=select(1.,power_weight(pdf,bsdf.pdf),continuation && !visibility.glass);
        radiance+=visibility.transmission*bsdf.value*
            spectral_environment_value(sky_direction,band)*PI*weight;
    }
    if frame.sun.w>0. {
        let phi=random()*2.*PI;
        let ct=mix(cos(frame.optical.y),1.,random());
        let st=sqrt(max(0.,1.-ct*ct));
        let d=basis(frame.sun.xyz,vec3<f32>(st*cos(phi),st*sin(phi),ct));
        let cosine=max(0.,dot(n,d));
        if cosine>0. && dot(ng,d)>0. {
            let visibility=shadow_transmittance_spectral_scalar(
                p+ng*EPS*4.,d,512.,band);
            let bsdf=eval_bsdf_spectral_scalar(wo,d,n,albedo,surface,band);
            let weight=select(1.,power_weight(sun_pdf(),bsdf.pdf),
                continuation && !visibility.glass);
            radiance+=visibility.transmission*bsdf.value*
                spectral_sun_value(band)*cosine*weight;
        }
    }
    if frame.options.y>0u {
        let light=sample_light();
        let light_material=materials[light.cell.w];
        let tri=triangles[light_material.range.x+
            min(u32(random()*f32(light_material.range.y)),light_material.range.y-1u)];
        let u=sqrt(random());
        let v=random();
        let b=vec2<f32>(u*(1.-v),u*v);
        let light_position=vec3<f32>(light.cell.xyz)+tri.p0.xyz+
            tri.e1.xyz*b.x+tri.e2.xyz*b.y;
        let delta=light_position-p;
        let distance=length(delta);
        let d=delta/max(distance,EPS);
        let light_ng=cross(tri.e1.xyz,tri.e2.xyz);
        let area=length(light_ng)*0.5;
        let cos_l=abs(dot(normalize(light_ng),-d));
        let cos_s=max(0.,dot(n,d));
        if cos_s>0. && dot(ng,d)>0. && cos_l>0. && distance>EPS*8. {
            let visibility=shadow_transmittance_spectral_scalar(
                p+ng*EPS*4.,d,distance-EPS*8.,band);
            let connection_cos_s=max(0.,dot(n,d));
            let connection_cos_ng=max(0.,dot(ng,d));
            if visibility.transmission>0. && connection_cos_s>0. && connection_cos_ng>0. {
                let uv=tri.uv0_uv1.xy*(1.-b.x-b.y)+tri.uv0_uv1.zw*b.x+
                    tri.uv2_layer.xy*b.y;
                let layer=ctm_layer(tri,vec3<i32>(light.cell.xyz));
                let color=texel(layer,uv);
                if color.a>=0.1 {
                    let le=spectral_value(color.rgb*
                        mix(vec3<f32>(1.),light_material.tint_emission.rgb,tri.p0.w),band)*
                        surface_emission(light_material,tri,uv,layer);
                    let pdf=distance*distance*light.sampling.y/
                        max(cos_l*area*f32(light_material.range.y),1e-12);
                    let bsdf=eval_bsdf_spectral_scalar(wo,d,n,albedo,surface,band);
                    let weight=select(1.,power_weight(pdf,bsdf.pdf),
                        continuation && !visibility.glass);
                    radiance+=visibility.transmission*bsdf.value*le*connection_cos_s/
                        max(pdf,1e-12)*weight;
                }
            }
        }
    }
    return radiance;
}
fn trace_frame_spectral(id: vec3<u32>) {
    let band=spectral_band(id);
    rng=(id.x+id.y*frame.image.x)*9781u+frame.image.z*6271u+
        band*911u+3812015801u;
    // Four successive accumulation strata cover 360..830 nm uniformly. The
    // intra-stratum jitter keeps the estimator continuous while the schedule
    // makes the first four samples cover the visible spectrum deterministically.
    hero_wavelength=360.+(f32(band)+random())*(470./4.);
    var d=primary_direction(vec2<f32>(id.xy)+vec2<f32>(random(),random()));
    var p=frame.camera.xyz;
    var throughput=1.;
    var radiance=0.;
    var guide=vec4<f32>(0.,0.,0.,-1.);
    var first: Hit;
    var previous_pdf=0.;
    var previous_normal=vec3<f32>(0.);
    var previous_delta=true;
    var transmitted_connection=false;
    var current_medium=spectral_medium_at_scalar(p+d*EPS*4.,band);
    var previous_state=0xffffffffu;
    var surface_bounces=0u;
    for (var bounce=0u;bounce<frame.options.x+32u;bounce++) {
        let segment_medium=current_medium;
        let hit=trace_segment(p,d,512.,previous_state);
        previous_state=hit.state;
        if bounce==0u { first=hit; }
        if hit.t<0. {
            var sky_weight=select(power_weight(previous_pdf,
                max(0.,dot(previous_normal,d))/PI),1.,previous_delta);
            if transmitted_connection { sky_weight=0.; }
            radiance+=throughput*spectral_environment_value(d,band)*sky_weight;
            if frame.sun.w>0. && dot(d,frame.sun.xyz)>cos(frame.optical.y) {
                var weight=select(power_weight(previous_pdf,sun_pdf()),1.,previous_delta);
                if transmitted_connection { weight=0.; }
                radiance+=throughput*spectral_sun_value(band)*weight;
            }
            break;
        }
        throughput*=exp(-segment_medium.absorption*hit.t);
        p+=d*hit.t;
        if bounce==0u { guide=vec4<f32>(hit.shading_normal,hit.t); }
        let mat=materials[hit.state];
        if hit.emission>0. {
            var weight=1.;
            if transmitted_connection && light_probability(hit.cell)>0. {
                weight=0.;
            } else if !previous_delta && frame.options.y>0u {
                let cosine=abs(dot(hit.normal,-d));
                let light_pdf=hit.t*hit.t*light_probability(hit.cell)/
                    max(cosine*hit.area*f32(mat.range.y),1e-12);
                weight=power_weight(previous_pdf,light_pdf);
            }
            radiance+=throughput*spectral_value(hit.albedo,band)*hit.emission*weight;
        }
        if mat.surface.z>0.5 {
            let thin=mat.absorption.w>0.5;
            if thin && hit.front==0u {
                p+=d*EPS*4.;
                continue;
            }
            let media=interface_media(p,hit,Medium(
                current_medium.ior,vec3<f32>(current_medium.absorption)));
            let eta_i=select(media.x,1.,thin);
            let eta_t=select(media.y,mat.surface.w,thin);
            let ci=clamp(dot(-d,hit.normal),0.,1.);
            let fresnel=spectral_fresnel_value(ci,eta_i,eta_t,band);
            let reflect_probability=clamp(fresnel,0.02,0.98);
            if random()<reflect_probability {
                d=reflect(d,hit.normal);
                throughput*=fresnel/reflect_probability;
            } else if thin {
                throughput*=(1.-fresnel)/(1.-reflect_probability);
                throughput*=exp(-spectral_absorption_value(
                    mat.absorption.rgb,band)*0.0625);
            } else {
                let eta=spectral_ior(eta_i,band)/spectral_ior(eta_t,band);
                let refracted=refract(d,hit.normal,eta);
                if length(refracted)<1e-4 { break; }
                d=normalize(refracted);
                throughput*=(1.-fresnel)/(1.-reflect_probability);
                throughput*=eta*eta;
                current_medium=spectral_medium_at_scalar(p+d*EPS*8.,band);
            }
            previous_pdf=0.;
            previous_delta=true;
            transmitted_connection=false;
            p+=d*EPS*4.;
        } else {
            let wo=-d;
            let continuation=surface_bounces+1u<frame.options.x;
            radiance+=throughput*direct_light_spectral_scalar(
                p,hit.normal,hit.shading_normal,wo,hit.albedo,hit.surface,
                continuation,band);
            surface_bounces+=1u;
            if surface_bounces>=frame.options.x { break; }
            d=sample_bsdf(wo,hit.shading_normal,hit.surface);
            if dot(hit.normal,d)<=0. { break; }
            let bsdf=eval_bsdf_spectral_scalar(
                wo,d,hit.shading_normal,hit.albedo,hit.surface,band);
            if bsdf.pdf<=0. { break; }
            throughput*=bsdf.value*max(0.,dot(hit.shading_normal,d))/bsdf.pdf;
            previous_pdf=bsdf.pdf;
            previous_normal=hit.shading_normal;
            previous_delta=false;
            transmitted_connection=true;
            p+=hit.normal*EPS*4.;
        }
        if mat.surface.z<0.5 && surface_bounces>=5u {
            let probability=clamp(sqrt(max(throughput,0.)),0.25,1.);
            if random()>probability { break; }
            throughput/=probability;
        }
    }
    if frame.options.w>0u {
        let stable=trace(frame.camera.xyz,
            primary_direction(vec2<f32>(id.xy)+vec2<f32>(0.5)),512.);
        guide=vec4<f32>(stable.shading_normal,stable.t);
    }
    var converted=vec3<f32>(0.);
    if frame.options.z==0u {
        converted=spectral_hero_to_rgb(radiance);
    } else if frame.options.z==1u {
        converted=first.shading_normal*0.5+0.5;
    } else if frame.options.z==2u {
        converted=vec3<f32>(max(0.,first.t)/128.);
    } else if frame.options.z==3u {
        converted=vec3<f32>(f32(first.steps)/128.,f32(first.steps)/384.,0.);
    } else if frame.options.z==4u {
        converted=first.surface.xyz;
    } else if frame.options.z==5u {
        converted=first.albedo;
    }
    let count=f32(frame.image.w);
    var previous_color=vec3<f32>(0.);
    if frame.image.w>0u { previous_color=textureLoad(previous,vec2<i32>(id.xy),0).rgb; }
    let mean=mix(previous_color,converted,1./(count+1.));
    textureStore(result,vec2<i32>(id.xy),vec4<f32>(mean,1.));
    textureStore(guides,vec2<i32>(id.xy),guide);
}

@compute @workgroup_size(8,8)
fn trace_frame(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy>=frame.image.xy) { return; }
    if frame.optical.w>0.5 {
        trace_frame_spectral(id);
        return;
    }
    rng=(id.x+id.y*frame.image.x)*9781u+frame.image.z*6271u+3812015801u;
    var d=primary_direction(vec2<f32>(id.xy)+vec2<f32>(random(),random()));
    var p=frame.camera.xyz;
    var throughput=vec3<f32>(1.);
    var radiance=vec3<f32>(0.);
    var guide=vec4<f32>(0.,0.,0.,-1.);
    var first: Hit;
    var previous_pdf=0.;
    var previous_normal=vec3<f32>(0.);
    var previous_delta=true;
    var nee_connection=false;
    var transmitted_connection=false;
    var eta_scale=1.;
    var current_medium=medium_at(p+d*EPS*4.);
    var previous_state=0xffffffffu;
    var surface_bounces=0u;
    // Glass interfaces have their own finite budget, so a few window layers
    // cannot consume every diffuse bounce and turn the view behind them black.
    for (var bounce=0u;bounce<frame.options.x+32u;bounce++) {
        let segment_medium=current_medium;
        let absorption=segment_medium.absorption;
        let hit=trace_segment(p,d,512.,previous_state);
        previous_state=hit.state;
        if bounce==0u { first=hit; }
        if hit.t<0. {
            var sky_weight=select(power_weight(previous_pdf,max(0.,dot(previous_normal,d))/PI),1.,previous_delta);
            if transmitted_connection { sky_weight=0.; }
            radiance += throughput*environment(d)*sky_weight;
            if frame.sun.w>0. && dot(d,frame.sun.xyz)>cos(frame.optical.y) {
                var weight=select(power_weight(previous_pdf,sun_pdf()),1.,previous_delta);
                if transmitted_connection { weight=0.; }
                radiance += throughput*sun_radiance()*weight;
            }
            break;
        }
        throughput *= exp(-absorption*hit.t);
        p += d*hit.t;
        if bounce==0u {
            guide=vec4<f32>(hit.shading_normal,hit.t);
        }
        let mat=materials[hit.state];
        if hit.emission>0. {
            var weight=1.;
            if transmitted_connection && light_probability(hit.cell)>0. {
                // Direct light through glass belongs exclusively to NEE.
                // For solid glass this deliberately disables refractive
                // caustics in favour of the straight-connection approximation.
                weight=0.;
            } else if !previous_delta && frame.options.y>0u {
                let cosine=abs(dot(hit.normal,-d));
                let light_pdf=hit.t*hit.t*light_probability(hit.cell)/max(cosine*hit.area*f32(mat.range.y),1e-12);
                weight=power_weight(previous_pdf,light_pdf);
            }
            radiance += throughput*hit.albedo*hit.emission*weight;
        }
        if mat.surface.z>0.5 {
            let thin=mat.absorption.w>0.5;
            if thin && hit.front==0u {
                p += d*EPS*4.;
                continue;
            }
            let media=interface_media(p,hit,current_medium);
            let eta_i=select(media.x,1.,thin);
            let eta_t=select(media.y,mat.surface.w,thin);
            let ci=clamp(dot(-d,hit.normal),0.,1.);
            var fresnel=fresnel_dielectric(ci,eta_i,select(eta_t,mat.surface.w,thin));
            if thin { fresnel=2.*fresnel/(1.+fresnel); }
            if random()<fresnel {
                d=reflect(d,hit.normal);
                // Reflected chains cannot be generated by a straight shadow ray.
                nee_connection=false;
                transmitted_connection=false;
            } else if thin {
                throughput *= exp(-mat.absorption.rgb*0.0625);
                transmitted_connection=nee_connection;
            } else {
                let eta=eta_i/eta_t;
                d=refract(d,hit.normal,eta);
                throughput *= eta*eta;
                eta_scale /= eta*eta;
                transmitted_connection=nee_connection;
                current_medium=medium_at(p+d*EPS*8.);
            }
            previous_pdf=0.;
            previous_delta=true;
            p += d*EPS*4.;
        } else {
            let wo=-d;
            let continuation=surface_bounces+1u<frame.options.x;
            radiance += throughput*direct_light(p,hit.normal,hit.shading_normal,wo,hit.albedo,hit.surface,continuation);
            surface_bounces += 1u;
            if surface_bounces>=frame.options.x { break; }
            d=sample_bsdf(wo,hit.shading_normal,hit.surface);
            if dot(hit.normal,d)<=0. { break; }
            let bsdf=eval_bsdf(wo,d,hit.shading_normal,hit.albedo,hit.surface);
            if bsdf.pdf<=0. { break; }
            throughput *= bsdf.value*max(0.,dot(hit.shading_normal,d))/bsdf.pdf;
            previous_pdf=bsdf.pdf;
            previous_normal=hit.shading_normal;
            previous_delta=false;
            nee_connection=true;
            transmitted_connection=false;
            p += hit.normal*EPS*4.;
        }
        if mat.surface.z<0.5 && surface_bounces>=5u {
            // Later, gentler roulette retains more low-energy indirect paths.
            // Probability one must remain possible; forcing 5% rejection adds noise.
            let rr_throughput=throughput*eta_scale;
            let probability=clamp(sqrt(max(rr_throughput.r,max(rr_throughput.g,rr_throughput.b))),0.25,1.);
            if random()>probability { break; }
            throughput /= probability;
        }
    }
    if frame.options.w>0u {
        let stable=trace(frame.camera.xyz,primary_direction(vec2<f32>(id.xy)+vec2<f32>(0.5)),512.);
        guide=vec4<f32>(stable.shading_normal,stable.t);
    }
    if frame.options.z==1u { radiance=first.shading_normal*0.5+0.5; }
    if frame.options.z==2u { radiance=vec3<f32>(max(0.,first.t)/128.); }
    if frame.options.z==3u { radiance=vec3<f32>(f32(first.steps)/128.,f32(first.steps)/384.,0.); }
    if frame.options.z==4u { radiance=first.surface.xyz; }
    if frame.options.z==5u { radiance=first.albedo; }
    let count=f32(frame.image.w);
    var previous_color=vec3<f32>(0.);
    if frame.image.w>0u { previous_color=textureLoad(previous,vec2<i32>(id.xy),0).rgb; }
    let mean=mix(previous_color,radiance,1./(count+1.));
    textureStore(result,vec2<i32>(id.xy),vec4<f32>(mean,1.));
    textureStore(guides,vec2<i32>(id.xy),guide);
}

@group(1) @binding(0) var show_history: texture_2d<f32>;
@group(1) @binding(1) var show_guides: texture_2d<f32>;
struct Varying { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_show(@builtin(vertex_index) i: u32) -> Varying {
    let pos=array<vec2<f32>,3>(vec2<f32>(-1.,-1.),vec2<f32>(3.,-1.),vec2<f32>(-1.,3.))[i];
    var out: Varying;
    out.position=vec4<f32>(pos,0.,1.);
    out.uv=vec2<f32>(pos.x*0.5+0.5,0.5-pos.y*0.5);
    return out;
}
@fragment fn fs_show(in: Varying) -> @location(0) vec4<f32> {
    let extent=vec2<i32>(textureDimensions(show_history));
    let pixel=clamp(vec2<i32>(in.uv*vec2<f32>(extent)),vec2<i32>(0),extent-1);
    let center=textureLoad(show_guides,pixel,0);
    var sum=textureLoad(show_history,pixel,0).rgb;
    var weight=1.;
    if frame.options.w>0u && frame.options.z==0u {
        for (var y=-1;y<=1;y++) { for (var x=-1;x<=1;x++) {
            if x==0 && y==0 { continue; }
            let q=clamp(pixel+vec2<i32>(x,y),vec2<i32>(0),extent-1);
            let g=textureLoad(show_guides,q,0);
            let strength=clamp(1.-f32(frame.image.w)/256.,0.,1.);
            let w=strength*exp(-f32(x*x+y*y)*0.7)*pow(max(0.,dot(center.xyz,g.xyz)),32.)*exp(-abs(g.w-center.w)*2.);
            sum += textureLoad(show_history,q,0).rgb*w;
            weight += w;
        }}
    }
    var c=max(sum/weight*frame.optical.x,vec3<f32>(0.));
    if frame.options.z==0u {
        // Keep M4's toe and lower midtones exactly. The longer, C1-continuous
        // shoulder makes room for brighter physical lighting without lifting
        // black or clipping coloured emitter texture into a white patch.
        let peak=max(c.r,max(c.g,c.b));
        var mapped=0.98*(peak*(2.43*peak+0.03))/(peak*(2.43*peak+0.59)+0.14);
        if peak>0.25 {
            let knee=0.25;
            let denominator=knee*(2.43*knee+0.59)+0.14;
            let at_knee=0.98*knee*(2.43*knee+0.03)/denominator;
            let slope=0.98*(2.43*0.56*knee*knee+2.*2.43*0.14*knee+0.03*0.14)/(denominator*denominator);
            let headroom=0.98-at_knee;
            let width=headroom*0.35/slope;
            mapped=at_knee+headroom*(1.-pow(1.+(peak-knee)/width,-0.35));
        }
        c *= mapped/max(peak,1e-12);
        c=select(12.92*c,1.055*pow(c,vec3<f32>(1./2.4))-0.055,c>vec3<f32>(0.0031308));
    }
    return vec4<f32>(c,1.);
}


// Offline PT Lite capture auxiliaries. Only the F2 capture entry point writes these.
@group(2) @binding(3) var native_capture_albedo: texture_storage_2d<rgba32float, write>;
@group(2) @binding(4) var native_capture_normal: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(8,8)
fn native_capture_guides(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy>=frame.image.xy) { return; }
    let hit=trace(frame.camera.xyz,
        primary_direction(vec2<f32>(id.xy)+vec2<f32>(0.5)),512.);
    var albedo=vec3<f32>(0.);
    var normal=vec3<f32>(0.);
    var covered=0.;
    if hit.t>=0. {
        albedo=clamp(hit.albedo,vec3<f32>(0.),vec3<f32>(1.));
        let square_length=dot(hit.shading_normal,hit.shading_normal);
        if square_length>1e-12 {
            normal=hit.shading_normal*inverseSqrt(square_length);
        }
        covered=1.;
    }
    textureStore(native_capture_albedo,vec2<i32>(id.xy),vec4<f32>(albedo,covered));
    textureStore(native_capture_normal,vec2<i32>(id.xy),vec4<f32>(normal,covered));
}
