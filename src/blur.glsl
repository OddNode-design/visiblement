// Frosted glass over the video: blurs the picture under the page's open menus, dialogs and player
// buttons. The page draws them in a WebView above mpv's window, so CSS backdrop-filter cannot reach the
// video; the page sends their rectangles here instead: up to five (a to e), as fractions of the scaled
// picture (x / y / width / height, top-left origin; a width of 0 means unused), with their corner
// radius in pixels. DYNAMIC: a change only updates a uniform, the shader is not rebuilt.

//!PARAM ax
//!TYPE DYNAMIC float
0.0
//!PARAM ay
//!TYPE DYNAMIC float
0.0
//!PARAM aw
//!TYPE DYNAMIC float
0.0
//!PARAM ah
//!TYPE DYNAMIC float
0.0
//!PARAM ar
//!TYPE DYNAMIC float
0.0
//!PARAM bx
//!TYPE DYNAMIC float
0.0
//!PARAM by
//!TYPE DYNAMIC float
0.0
//!PARAM bw
//!TYPE DYNAMIC float
0.0
//!PARAM bh
//!TYPE DYNAMIC float
0.0
//!PARAM br
//!TYPE DYNAMIC float
0.0
//!PARAM cx
//!TYPE DYNAMIC float
0.0
//!PARAM cy
//!TYPE DYNAMIC float
0.0
//!PARAM cw
//!TYPE DYNAMIC float
0.0
//!PARAM ch
//!TYPE DYNAMIC float
0.0
//!PARAM cr
//!TYPE DYNAMIC float
0.0
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
28.0

//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Visiblement menu blur

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
    float d = min(min(box(p, ax, ay, aw, ah, ar), box(p, bx, by, bw, bh, br)),
              min(min(box(p, cx, cy, cw, ch, cr), box(p, dx, dy, dw, dh, dr)), box(p, ex, ey, ew, eh, er)));
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
