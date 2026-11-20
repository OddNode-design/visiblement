// 360° videos (equirectangular): shows the part of the sphere the viewer looks at.
// yaw / pitch / fov (radians) come from the page through glsl-shader-opts.

//!PARAM yaw
//!TYPE float
0.0

//!PARAM pitch
//!TYPE float
0.0

//!PARAM fov
//!TYPE float
1.5708

//!HOOK MAIN
//!BIND HOOKED
//!WIDTH OUTPUT.w
//!HEIGHT OUTPUT.h
//!DESC Visiblement 360
vec4 hook() {
    vec2 uv = HOOKED_pos * 2.0 - 1.0;
    float t = tan(fov * 0.5);
    vec3 d = normalize(vec3(uv.x * t * target_size.x / target_size.y, -uv.y * t, 1.0));
    float cp = cos(pitch), sp = sin(pitch);
    d = vec3(d.x, d.y * cp + d.z * sp, d.z * cp - d.y * sp);
    float cy = cos(yaw), sy = sin(yaw);
    d = vec3(d.x * cy + d.z * sy, d.y, d.z * cy - d.x * sy);
    float lon = atan(d.x, d.z), lat = asin(clamp(d.y, -1.0, 1.0));
    return HOOKED_tex(vec2(fract(lon / 6.28318531 + 0.5), 0.5 - lat / 3.14159265));
}
