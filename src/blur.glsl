// Frosted glass over the video: blurs the picture under the page's open menus and dialogs.
// The page draws its menus in a WebView above mpv's window, so CSS backdrop-filter cannot reach the
// video; the page sends their rectangles here instead (up to three, as fractions of the window,
// x / y / width / height, top-left origin; a width of 0 means unused) and their corner radius in px.

//!PARAM ax
//!TYPE float
0.0
//!PARAM ay
//!TYPE float
0.0
//!PARAM aw
//!TYPE float
0.0
//!PARAM ah
//!TYPE float
0.0
//!PARAM bx
//!TYPE float
0.0
//!PARAM by
//!TYPE float
0.0
//!PARAM bw
//!TYPE float
0.0
//!PARAM bh
//!TYPE float
0.0
//!PARAM cx
//!TYPE float
0.0
//!PARAM cy
//!TYPE float
0.0
//!PARAM cw
//!TYPE float
0.0
//!PARAM ch
//!TYPE float
0.0
//!PARAM corner
//!TYPE float
12.0
//!PARAM radius
//!TYPE float
28.0

//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Visiblement menu blur

// Signed distance (px) from p to a rounded rectangle given as fractions of the output.
float box(vec2 p, float x, float y, float w, float h) {
    if (w <= 0.0) return 1e9;
    vec2 size = vec2(w, h) * HOOKED_size;
    vec2 centre = vec2(x, y) * HOOKED_size + size * 0.5;
    vec2 q = abs(p - centre) - size * 0.5 + corner;
    return length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - corner;
}

vec4 hook() {
    vec2 p = HOOKED_pos * HOOKED_size;
    float d = min(box(p, ax, ay, aw, ah), min(box(p, bx, by, bw, bh), box(p, cx, cy, cw, ch)));
    if (d > 0.0) return HOOKED_tex(HOOKED_pos);
    // Disk blur: 64 taps on a golden-angle spiral, weighted toward the centre.
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int i = 0; i < 64; i++) {
        float r = sqrt((float(i) + 0.5) / 64.0);
        float a = float(i) * 2.39996323;
        float w = 1.0 - r * 0.6;
        sum += HOOKED_texOff(vec2(cos(a), sin(a)) * r * radius) * w;
        total += w;
    }
    return sum / total;
}
