//! The pixel marks of the arena badges, 9x9 each, in one flat style: a
//! chunky shape, a colour or two, no detail. Claude's, OpenAI's and Gemini's
//! are the reference icons the others were drawn to match. The panel code
//! is in [`crate::arenahud`].

/// Pixels a side.
pub const SIZE: usize = 9;

/// The ground every mark is drawn on.
pub const GROUND: u32 = 0xf4f1ea;

/// A mark's pixels, row by row: `0` for ground, else `0xRRGGBB`.
pub type Mark = [u32; SIZE * SIZE];

/// The mark of a badge's letters (see `arenahud::badge`).
pub fn mark(letters: &str) -> Option<&'static Mark> {
    match letters {
        "CL" => Some(&CL),
        "GP" => Some(&GP),
        "GE" => Some(&GE),
        "QW" => Some(&QW),
        "DS" => Some(&DS),
        "LL" => Some(&LL),
        "GK" => Some(&GK),
        "MI" => Some(&MI),
        "KI" => Some(&KI),
        "GL" => Some(&GL),
        _ => None,
    }
}

/// Claude: the reference icon.
#[rustfmt::skip]
static CL: Mark = [
    0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0xd97757, 0, 0, 0, 0,
    0, 0, 0xd97757, 0, 0xd97757, 0, 0xd97757, 0, 0,
    0, 0, 0, 0xd97757, 0xd97757, 0xd97757, 0, 0, 0,
    0, 0xd97757, 0xd97757, 0xd97757, 0xd97757, 0xd97757, 0xd97757, 0xd97757, 0,
    0, 0, 0, 0xd97757, 0xd97757, 0xd97757, 0, 0, 0,
    0, 0, 0xd97757, 0, 0xd97757, 0, 0xd97757, 0, 0,
    0, 0, 0, 0, 0xd97757, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// OpenAI: the reference icon.
#[rustfmt::skip]
static GP: Mark = [
    0, 0, 0x343341, 0x343341, 0, 0x343341, 0x343341, 0, 0,
    0, 0x343341, 0, 0, 0x343341, 0, 0, 0x343341, 0,
    0, 0x343341, 0, 0x343341, 0, 0, 0x343341, 0x343341, 0x343341,
    0x343341, 0x343341, 0, 0x343341, 0x343341, 0x343341, 0, 0, 0x343341,
    0x343341, 0, 0x343341, 0x343341, 0, 0x343341, 0x343341, 0, 0x343341,
    0x343341, 0, 0, 0x343341, 0x343341, 0x343341, 0, 0x343341, 0x343341,
    0x343341, 0x343341, 0x343341, 0, 0, 0x343341, 0, 0x343341, 0,
    0, 0x343341, 0, 0, 0x343341, 0, 0, 0x343341, 0,
    0, 0, 0x343341, 0x343341, 0, 0x343341, 0x343341, 0, 0,
];

/// Gemini: the reference icon.
#[rustfmt::skip]
static GE: Mark = [
    0, 0, 0, 0, 0x7c72c6, 0, 0, 0, 0,
    0, 0, 0, 0, 0x7c72c6, 0, 0, 0, 0,
    0, 0, 0, 0x9168c0, 0x7c72c6, 0x438dd7, 0, 0, 0,
    0, 0, 0x9168c0, 0x7c72c6, 0x637fce, 0x438dd7, 0x1ba1e3, 0, 0,
    0x9168c0, 0x9168c0, 0x9168c0, 0x7c72c6, 0x637fce, 0x438dd7, 0x1ba1e3, 0x1ba1e3, 0x1ba1e3,
    0, 0, 0x7c72c6, 0x637fce, 0x438dd7, 0x1ba1e3, 0x1ba1e3, 0, 0,
    0, 0, 0, 0x637fce, 0x438dd7, 0x1ba1e3, 0, 0, 0,
    0, 0, 0, 0, 0x438dd7, 0, 0, 0, 0,
    0, 0, 0, 0, 0x438dd7, 0, 0, 0, 0,
];

/// Qwen: three angular arms turning round a triangle.
#[rustfmt::skip]
static QW: Mark = [
    0, 0, 0, 0x615ced, 0x615ced, 0, 0, 0, 0,
    0, 0, 0, 0x615ced, 0x615ced, 0x615ced, 0x615ced, 0, 0,
    0, 0x615ced, 0, 0, 0, 0, 0x615ced, 0, 0,
    0, 0x615ced, 0x615ced, 0, 0x615ced, 0, 0, 0, 0,
    0, 0x615ced, 0x615ced, 0, 0x615ced, 0x615ced, 0, 0x615ced, 0,
    0, 0, 0, 0, 0x615ced, 0, 0x615ced, 0x615ced, 0,
    0, 0, 0x615ced, 0, 0, 0, 0, 0x615ced, 0,
    0, 0, 0x615ced, 0x615ced, 0x615ced, 0x615ced, 0, 0, 0,
    0, 0, 0, 0, 0x615ced, 0x615ced, 0, 0, 0,
];

/// DeepSeek: a whale.
#[rustfmt::skip]
static DS: Mark = [
    0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0x4d6bfe, 0x4d6bfe,
    0, 0, 0, 0, 0, 0, 0x4d6bfe, 0x4d6bfe, 0,
    0, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0, 0x4d6bfe, 0, 0,
    0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0, 0,
    0x4d6bfe, 0, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0, 0,
    0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0, 0,
    0, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0x4d6bfe, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Meta: a loop.
#[rustfmt::skip]
static LL: Mark = [
    0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0x0866ff, 0x0866ff, 0, 0, 0, 0x0866ff, 0x0866ff, 0,
    0x0866ff, 0, 0, 0x0866ff, 0, 0x0866ff, 0, 0, 0x0866ff,
    0x0866ff, 0, 0, 0, 0x0866ff, 0, 0, 0, 0x0866ff,
    0x0866ff, 0, 0, 0x0866ff, 0, 0x0866ff, 0, 0, 0x0866ff,
    0, 0x0866ff, 0x0866ff, 0, 0, 0, 0x0866ff, 0x0866ff, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Grok: a ring cut by a slash.
#[rustfmt::skip]
static GK: Mark = [
    0, 0, 0, 0, 0, 0, 0, 0x1a1a1f, 0x1a1a1f,
    0, 0, 0x1a1a1f, 0x1a1a1f, 0x1a1a1f, 0x1a1a1f, 0, 0x1a1a1f, 0,
    0, 0x1a1a1f, 0, 0, 0, 0x1a1a1f, 0x1a1a1f, 0, 0,
    0x1a1a1f, 0, 0, 0, 0x1a1a1f, 0x1a1a1f, 0, 0, 0x1a1a1f,
    0x1a1a1f, 0, 0, 0x1a1a1f, 0x1a1a1f, 0, 0, 0, 0x1a1a1f,
    0x1a1a1f, 0, 0x1a1a1f, 0x1a1a1f, 0, 0, 0, 0, 0x1a1a1f,
    0, 0, 0x1a1a1f, 0x1a1a1f, 0, 0, 0, 0x1a1a1f, 0,
    0, 0x1a1a1f, 0, 0x1a1a1f, 0x1a1a1f, 0x1a1a1f, 0x1a1a1f, 0, 0,
    0x1a1a1f, 0x1a1a1f, 0, 0, 0, 0, 0, 0, 0,
];

/// Mistral: a banded M.
#[rustfmt::skip]
static MI: Mark = [
    0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0xffff00, 0, 0, 0, 0, 0, 0xffff00, 0,
    0, 0xffff00, 0xfff700, 0, 0, 0, 0xfff700, 0xffff00, 0,
    0, 0xffe600, 0xffa701, 0xffba01, 0, 0xffba01, 0xffa701, 0xffe600, 0,
    0, 0xffb105, 0xff8205, 0xff8205, 0xff8205, 0xff8205, 0xff8205, 0xffb105, 0,
    0, 0xff7410, 0xff8d13, 0, 0xfa560e, 0, 0xff8d13, 0xff7410, 0,
    0xff0800, 0xff2004, 0xff1e03, 0xff1501, 0, 0xff1501, 0xff1e03, 0xff2004, 0xff0800,
    0xff0500, 0xff0500, 0xff0500, 0xff0800, 0, 0xff0800, 0xff0500, 0xff0500, 0xff0500,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Kimi: a K and its dot.
#[rustfmt::skip]
static KI: Mark = [
    0, 0, 0, 0, 0, 0, 0, 0, 0x04f0ff,
    0x000001, 0x000001, 0, 0, 0, 0x000001, 0, 0x01caff, 0x0282ff,
    0x000001, 0x000001, 0, 0, 0x000001, 0x000001, 0, 0, 0,
    0x000001, 0x000001, 0, 0, 0x000001, 0x000001, 0, 0, 0,
    0x000001, 0x000001, 0x000001, 0x000001, 0x000001, 0x000001, 0x000001, 0, 0,
    0x000001, 0x000001, 0, 0, 0, 0, 0x000001, 0, 0,
    0x000001, 0x000001, 0, 0, 0, 0, 0x000001, 0, 0,
    0x000001, 0x000001, 0, 0, 0, 0, 0x000001, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Zhipu GLM: a spark.
#[rustfmt::skip]
static GL: Mark = [
    0, 0, 0, 0, 0x335cf0, 0, 0, 0, 0,
    0, 0, 0x335cf0, 0, 0x335cf0, 0, 0x335cf0, 0, 0,
    0, 0x335cf0, 0, 0x335cf0, 0x335cf0, 0x335cf0, 0, 0x335cf0, 0,
    0, 0, 0x335cf0, 0x335cf0, 0x335cf0, 0x335cf0, 0x335cf0, 0, 0,
    0x335cf0, 0x335cf0, 0x335cf0, 0x335cf0, 0, 0x335cf0, 0x335cf0, 0x335cf0, 0x335cf0,
    0, 0, 0x335cf0, 0x335cf0, 0x335cf0, 0x335cf0, 0x335cf0, 0, 0,
    0, 0x335cf0, 0, 0x335cf0, 0x335cf0, 0x335cf0, 0, 0x335cf0, 0,
    0, 0, 0x335cf0, 0, 0x335cf0, 0, 0x335cf0, 0, 0,
    0, 0, 0, 0, 0x335cf0, 0, 0, 0, 0,
];
