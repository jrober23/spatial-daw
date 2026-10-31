struct Uniforms {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct SphereIn {
    @location(0) center: vec3<f32>,
    @location(1) radius: f32,
    @location(2) color: vec4<f32>,
};

struct SphereOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) depth: f32,
};

@vertex
fn vs_sphere(@builtin(vertex_index) index: u32, instance: SphereIn) -> SphereOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>(-1.0,  1.0),
    );
    let corner = corners[index];
    let view_center = uniforms.view * vec4<f32>(instance.center, 1.0);
    let offset = vec3<f32>(corner * instance.radius * 1.25, 0.0);
    var out: SphereOut;
    out.clip = uniforms.proj * vec4<f32>(view_center.xyz + offset, 1.0);
    out.uv = corner;
    out.color = instance.color;
    out.depth = -view_center.z;
    return out;
}

@fragment
fn fs_sphere(input: SphereOut) -> @location(0) vec4<f32> {
    let radius_sq = dot(input.uv, input.uv);
    if (radius_sq > 1.0) {
        discard;
    }
    let z = sqrt(max(0.0, 1.0 - radius_sq));
    let normal = normalize(vec3<f32>(input.uv.x, -input.uv.y, z));
    let light = normalize(vec3<f32>(-0.45, 0.65, 0.85));
    let diffuse = max(dot(normal, light), 0.0);
    let rim = pow(1.0 - z, 2.2);
    let shade = 0.28 + diffuse * 0.8 + rim * 0.35;
    let fog = clamp(1.0 - (input.depth - 9.0) / 34.0, 0.2, 1.0);
    let alpha = input.color.a * (0.65 + rim * 0.35);
    return vec4<f32>(input.color.rgb * shade * fog, alpha);
}

struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(input: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = uniforms.view_proj * vec4<f32>(input.pos, 1.0);
    out.color = input.color;
    return out;
}

@fragment
fn fs_line(input: LineOut) -> @location(0) vec4<f32> {
    return input.color;
}
