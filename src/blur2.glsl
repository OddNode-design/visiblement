// Frosted glass over the video, panels d and e.
// The page draws its menus, dialogs, player buttons and messages in a WebView above mpv's window, so
// CSS backdrop-filter cannot reach the video; the page sends their rectangles here instead, as fractions
// of the scaled picture (x / y / width / height, top-left origin; a width of 0 means unused), with their
// corner radius and the blur radius in px. Two files of at most 16 parameters: mpv's older renderer
// (macOS) allows no more. DYNAMIC (Windows): a change only updates a uniform; macOS gets a copy without
// it (src/app.rs).
// The blur: the picture shrunk 8 times, a true Gaussian blur on that (horizontal then vertical), scaled
// back up under the panels. Smooth at any radius, and the cost does not grow with it.

//!PARAM dx
//!TYPE DYNAMIC float
0.0
//!PARAM dy
//!TYPE DYNAMIC float
0.0
//!PARAM dw
//!TYPE DYNAMIC float
0.0
//!PARAM dh
//!TYPE DYNAMIC float
0.0
//!PARAM dr
//!TYPE DYNAMIC float
0.0
//!PARAM ex
//!TYPE DYNAMIC float
0.0
//!PARAM ey
//!TYPE DYNAMIC float
0.0
//!PARAM ew
//!TYPE DYNAMIC float
0.0
//!PARAM eh
//!TYPE DYNAMIC float
0.0
//!PARAM er
//!TYPE DYNAMIC float
0.0
//!PARAM radius
//!TYPE DYNAMIC float
80.0

//!HOOK OUTPUT
//!BIND HOOKED
//!SAVE VBSMALL2
//!WIDTH OUTPUT.w 8 /
//!HEIGHT OUTPUT.h 8 /
//!DESC Visiblement menu blur 2: shrink
vec4 hook() {
    vec4 sum = vec4(0.0);
    for (int y = -3; y <= 3; y += 2)
        for (int x = -3; x <= 3; x += 2)
            sum += HOOKED_texOff(vec2(float(x), float(y)));
    return sum / 16.0;
}

//!HOOK OUTPUT
//!BIND VBSMALL2
//!SAVE VBBLURH2
//!WIDTH VBSMALL2.w
//!HEIGHT VBSMALL2.h
//!DESC Visiblement menu blur 2: gaussian
vec4 hook() {
    float sigma = max(radius / 16.0, 0.5);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int i = -24; i <= 24; i++) {
        float w = exp(-float(i * i) / (2.0 * sigma * sigma));
        sum += VBSMALL2_texOff(vec2(float(i), 0.0)) * w;
        total += w;
    }
    return sum / total;
}

//!HOOK OUTPUT
//!BIND VBBLURH2
//!SAVE VBBLURV2
//!WIDTH VBBLURH2.w
//!HEIGHT VBBLURH2.h
//!DESC Visiblement menu blur 2: gaussian
vec4 hook() {
    float sigma = max(radius / 16.0, 0.5);
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int i = -24; i <= 24; i++) {
        float w = exp(-float(i * i) / (2.0 * sigma * sigma));
        sum += VBBLURH2_texOff(vec2(0.0, float(i))) * w;
        total += w;
    }
    return sum / total;
}

//!HOOK OUTPUT
//!BIND HOOKED
//!BIND VBBLURV2
//!DESC Visiblement menu blur 2
// Signed distance (px) from p to a rounded rectangle given as fractions of the output.
float box(vec2 p, float x, float y, float w, float h, float r) {
    if (w <= 0.0) return 1e9;
    vec2 size = vec2(w, h) * HOOKED_size;
    r = min(r, 0.5 * min(size.x, size.y));
    vec2 q = abs(p - (vec2(x, y) * HOOKED_size + size * 0.5)) - size * 0.5 + r;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
}

vec4 hook() {
    vec2 p = HOOKED_pos * HOOKED_size;
    if (min(box(p, dx, dy, dw, dh, dr), box(p, ex, ey, ew, eh, er)) > 0.0) return HOOKED_tex(HOOKED_pos);
    return VBBLURV2_tex(HOOKED_pos);
}
