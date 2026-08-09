//! Minecraft skin decoding and 2D rendering.
//!
//! Produces two things the UI needs: the front of the head (the small avatar
//! next to a username) and a front-on full-body render.
//!
//! Skins are 64×64 pixel-art textures — occasionally still 64×32 for very old
//! accounts. Scaling is deliberately nearest-neighbour: any smoothing turns
//! pixel art into mush.
//!
//! Every part has an *overlay* layer (hat, jacket, sleeves, trouser legs)
//! drawn over the base with alpha. Skipping those is the usual reason a
//! rendered skin looks subtly wrong — hair and jacket detail simply vanish.

use crate::auth::SkinModel;
use crate::error::{Error, Result};

/// An RGBA8 image, ready to hand to the UI layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    /// Row-major, 4 bytes per pixel.
    pub pixels: Vec<u8>,
}

impl Rgba {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    fn set(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let index = ((y * self.width + x) * 4) as usize;
        self.pixels[index..index + 4].copy_from_slice(&rgba);
    }

    fn get(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let index = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        ]
    }

    /// Alpha-composites one pixel over whatever is already there — how
    /// overlay layers get applied.
    fn blend(&mut self, x: u32, y: u32, src: [u8; 4]) {
        if src[3] == 0 {
            return;
        }
        if src[3] == 255 {
            self.set(x, y, src);
            return;
        }

        let dst = self.get(x, y);
        let sa = src[3] as f32 / 255.0;
        let da = dst[3] as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            self.set(x, y, [0, 0, 0, 0]);
            return;
        }

        let mut out = [0u8; 4];
        for i in 0..3 {
            let s = src[i] as f32 / 255.0;
            let d = dst[i] as f32 / 255.0;
            out[i] = (((s * sa + d * da * (1.0 - sa)) / out_a) * 255.0).round() as u8;
        }
        out[3] = (out_a * 255.0).round() as u8;
        self.set(x, y, out);
    }

    /// Nearest-neighbour upscale by an integer factor.
    fn scaled(&self, factor: u32) -> Rgba {
        if factor <= 1 {
            return self.clone();
        }
        let mut out = Rgba::new(self.width * factor, self.height * factor);
        for y in 0..self.height {
            for x in 0..self.width {
                let pixel = self.get(x, y);
                for dy in 0..factor {
                    for dx in 0..factor {
                        out.set(x * factor + dx, y * factor + dy, pixel);
                    }
                }
            }
        }
        out
    }
}

/// A decoded skin texture.
pub struct Skin {
    texture: Rgba,
    model: SkinModel,
    /// Old 64×32 skins have no left-limb regions, so the right ones get
    /// mirrored instead.
    legacy: bool,
}

impl Skin {
    pub fn decode(png: &[u8], model: SkinModel) -> Result<Self> {
        let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(|err| Error::invalid(format!("could not read skin image: {err}")))?
            .to_rgba8();

        let (width, height) = decoded.dimensions();
        if width != 64 || (height != 64 && height != 32) {
            return Err(Error::invalid(format!(
                "unexpected skin dimensions {width}×{height}, expected 64×64"
            )));
        }

        Ok(Self {
            texture: Rgba {
                width,
                height,
                pixels: decoded.into_raw(),
            },
            model,
            legacy: height == 32,
        })
    }

    /// The raw texture, for callers that map it onto their own geometry
    /// rather than using the 2D renders here.
    pub fn texture(&self) -> &Rgba {
        &self.texture
    }

    /// The model this texture is actually laid out for.
    ///
    /// The profile's `variant` is metadata and can be wrong or stale; the
    /// texture cannot. Getting it wrong is not subtle: rendering a slim
    /// texture with classic geometry samples column 47 of each arm, which
    /// slim skins leave empty, so the arms come out full of holes.
    ///
    /// Detection uses the back of the right arm. Classic occupies x 52..56;
    /// slim only reaches 54, so 54..56 is always empty on a slim skin and
    /// essentially never empty on a classic one.
    pub fn detect_model(&self) -> SkinModel {
        if self.legacy {
            // 64x32 skins predate slim entirely.
            return SkinModel::Classic;
        }

        let empty = (20..32)
            .all(|y| (54..56).all(|x| self.texture.get(x, y)[3] == 0));

        if empty {
            SkinModel::Slim
        } else {
            SkinModel::Classic
        }
    }

    /// Re-reads the model from the texture, so later renders use the layout
    /// the pixels are actually in.
    pub fn use_detected_model(&mut self) -> SkinModel {
        self.model = self.detect_model();
        self.model
    }

    /// Width of one arm: slim skins use 3px arms instead of 4.
    fn arm_width(&self) -> u32 {
        match self.model {
            SkinModel::Classic => 4,
            SkinModel::Slim => 3,
        }
    }

    /// Copies a rectangle out of the texture, optionally mirrored (needed
    /// when synthesising left limbs from right ones on legacy skins).
    fn part(&self, x: u32, y: u32, w: u32, h: u32, mirror: bool) -> Rgba {
        let mut out = Rgba::new(w, h);
        for row in 0..h {
            for col in 0..w {
                let src_col = if mirror { w - 1 - col } else { col };
                out.set(col, row, self.texture.get(x + src_col, y + row));
            }
        }
        out
    }

    fn draw(&self, target: &mut Rgba, part: &Rgba, at_x: u32, at_y: u32) {
        for y in 0..part.height {
            for x in 0..part.width {
                target.blend(at_x + x, at_y + y, part.get(x, y));
            }
        }
    }

    /// The front of the head with the hat layer composited on, scaled up.
    /// This is the avatar shown beside a username.
    pub fn face(&self, scale: u32) -> Rgba {
        let mut face = self.part(8, 8, 8, 8, false);

        // Hat layer. Present on every modern skin and often carries the hair.
        let hat = self.part(40, 8, 8, 8, false);
        for y in 0..8 {
            for x in 0..8 {
                face.blend(x, y, hat.get(x, y));
            }
        }

        face.scaled(scale)
    }

    /// Front-on full body: head, torso, both arms, both legs, each with its
    /// overlay layer.
    pub fn body(&self, scale: u32) -> Rgba {
        let arm = self.arm_width();
        // 8-wide torso flanked by an arm on each side; 8 head + 12 torso + 12 legs.
        let width = 8 + arm * 2;
        let height = 32;
        let mut canvas = Rgba::new(width, height);

        // Torso sits centred, so everything else is placed relative to it.
        let torso_x = arm;

        // Head, centred over the torso.
        let head = self.part(8, 8, 8, 8, false);
        self.draw(&mut canvas, &head, torso_x, 0);
        let hat = self.part(40, 8, 8, 8, false);
        self.draw(&mut canvas, &hat, torso_x, 0);

        // Torso + jacket.
        let torso = self.part(20, 20, 8, 12, false);
        self.draw(&mut canvas, &torso, torso_x, 8);
        if !self.legacy {
            let jacket = self.part(20, 36, 8, 12, false);
            self.draw(&mut canvas, &jacket, torso_x, 8);
        }

        // The player's right arm/leg appear on the viewer's left.
        let right_arm = self.part(44, 20, arm, 12, false);
        self.draw(&mut canvas, &right_arm, 0, 8);
        if !self.legacy {
            let right_sleeve = self.part(44, 36, arm, 12, false);
            self.draw(&mut canvas, &right_sleeve, 0, 8);
        }

        // Legacy skins store only one arm and one leg; the other side is a
        // mirror image, which is exactly how the game rendered them.
        let left_arm = if self.legacy {
            self.part(44, 20, arm, 12, true)
        } else {
            self.part(36, 52, arm, 12, false)
        };
        self.draw(&mut canvas, &left_arm, torso_x + 8, 8);
        if !self.legacy {
            let left_sleeve = self.part(52, 52, arm, 12, false);
            self.draw(&mut canvas, &left_sleeve, torso_x + 8, 8);
        }

        let right_leg = self.part(4, 20, 4, 12, false);
        self.draw(&mut canvas, &right_leg, torso_x, 20);
        if !self.legacy {
            let right_pant = self.part(4, 36, 4, 12, false);
            self.draw(&mut canvas, &right_pant, torso_x, 20);
        }

        let left_leg = if self.legacy {
            self.part(4, 20, 4, 12, true)
        } else {
            self.part(20, 52, 4, 12, false)
        };
        self.draw(&mut canvas, &left_leg, torso_x + 4, 20);
        if !self.legacy {
            let left_pant = self.part(4, 52, 4, 12, false);
            self.draw(&mut canvas, &left_pant, torso_x + 4, 20);
        }

        canvas.scaled(scale)
    }
}

/// Downloads a skin texture and decodes it.
pub async fn fetch(http: &reqwest::Client, url: &str, model: SkinModel) -> Result<Skin> {
    Skin::decode(&fetch_png(http, url).await?, model)
}

/// Downloads a skin texture without decoding it.
///
/// The undecoded PNG is what gets kept in the skin library and re-uploaded
/// later, so it has to survive the round trip byte for byte rather than being
/// re-encoded from pixels.
pub async fn fetch_png(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let bytes = http.get(url).send().await?.error_for_status()?.bytes().await?;
    Ok(bytes.to_vec())
}

/// Crops a cape texture down to the panel that actually hangs off the
/// player's back, scaled up.
///
/// A cape texture is 64×32 and mostly empty; the visible outer face is the
/// 10×16 block at (1, 1). Showing the whole texture as a thumbnail is mostly
/// blank space with the design squashed into a corner.
pub fn cape_panel(texture: &Rgba, scale: u32) -> Rgba {
    const X: u32 = 1;
    const Y: u32 = 1;
    const W: u32 = 10;
    const H: u32 = 16;

    let mut panel = Rgba::new(W, H);
    for y in 0..H {
        for x in 0..W {
            panel.set(x, y, texture.get(X + x, Y + y));
        }
    }
    panel.scaled(scale)
}

/// Downloads any image and decodes it to RGBA, without the 64×64 shape check
/// [`Skin::decode`] applies.
///
/// Used for capes (64×32, with their own layout) and for Modrinth project
/// icons, which are variously PNG, WebP or JPEG — hence sniffing the format
/// from the bytes rather than assuming one.
pub async fn fetch_texture(http: &reqwest::Client, url: &str) -> Result<Rgba> {
    let bytes = http.get(url).send().await?.error_for_status()?.bytes().await?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|err| Error::invalid(format!("could not read texture: {err}")))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    Ok(Rgba {
        width,
        height,
        pixels: decoded.into_raw(),
    })
}

/// Stand-in shown before sign-in.
///
/// A brand-styled silhouette rather than Minecraft's Steve, because that
/// texture is Mojang's asset and this project ships none. It's generated in
/// the same body proportions as a real render, so swapping in a fetched
/// default skin later is a change of source, not of layout — see
/// [`placeholder_face`] for the matching avatar.
pub fn placeholder_body(scale: u32) -> Rgba {
    let mut canvas = Rgba::new(16, 32);

    // Head, torso, arms and legs, in the same geometry `Skin::body` uses.
    let regions: [(u32, u32, u32, u32); 6] = [
        (4, 0, 8, 8),   // head
        (4, 8, 8, 12),  // torso
        (0, 8, 4, 12),  // right arm
        (12, 8, 4, 12), // left arm
        (4, 20, 4, 12), // right leg
        (8, 20, 4, 12), // left leg
    ];

    for (x, y, w, h) in regions {
        for py in y..y + h {
            for px in x..x + w {
                canvas.set(px, py, gradient(py, 32));
            }
        }
    }

    canvas.scaled(scale)
}

/// A full 64×64 skin texture in the brand gradient, for the signed-out state
/// of the 3D viewer.
///
/// Returning a real skin-shaped texture rather than a special case means the
/// renderer has exactly one path: it always has a skin to map onto the model.
/// The gradient runs down the texture, so it reads as a vertical ramp on the
/// standing figure.
pub fn placeholder_texture() -> Rgba {
    let mut texture = Rgba::new(64, 64);
    for y in 0..64 {
        // Sampled across the head-to-feet span rather than the texture's own
        // height, so the ramp doesn't restart on each unwrapped part.
        let colour = gradient(y, 64);
        for x in 0..64 {
            texture.set(x, y, colour);
        }
    }
    texture
}

/// Avatar counterpart to [`placeholder_body`], so a signed-out state looks
/// consistent wherever a face would normally appear.
pub fn placeholder_face(scale: u32) -> Rgba {
    let mut canvas = Rgba::new(8, 8);
    for y in 0..8 {
        for x in 0..8 {
            canvas.set(x, y, gradient(y, 8));
        }
    }
    canvas.scaled(scale)
}

/// Vertical violet-to-magenta ramp, sampled from the logo's gradient stops.
fn gradient(y: u32, height: u32) -> [u8; 4] {
    let t = y as f32 / (height.max(1) - 1).max(1) as f32;
    let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    [
        lerp(0x7b, 0xff),
        lerp(0x3c, 0x3c),
        lerp(0xff, 0xac),
        0xff,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a 64×64 skin where every pixel is opaque red, so any region
    /// copied out of it is easy to assert on.
    fn solid_skin() -> Vec<u8> {
        let mut image = image::RgbaImage::new(64, 64);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([255, 0, 0, 255]);
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn face_is_eight_by_eight_before_scaling() {
        let skin = Skin::decode(&solid_skin(), SkinModel::Classic).unwrap();
        let face = skin.face(1);
        assert_eq!((face.width, face.height), (8, 8));
        assert_eq!(face.get(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn scaling_is_nearest_neighbour() {
        let skin = Skin::decode(&solid_skin(), SkinModel::Classic).unwrap();
        let face = skin.face(8);
        assert_eq!((face.width, face.height), (64, 64));
        // A blur would have produced intermediate values at the edges.
        assert_eq!(face.get(63, 63), [255, 0, 0, 255]);
    }

    #[test]
    fn slim_model_is_two_pixels_narrower() {
        let png = solid_skin();
        let classic = Skin::decode(&png, SkinModel::Classic).unwrap().body(1);
        let slim = Skin::decode(&png, SkinModel::Slim).unwrap().body(1);
        assert_eq!(classic.width, 16);
        assert_eq!(slim.width, 14);
        assert_eq!(classic.height, slim.height);
    }

    /// Builds a skin whose right-arm back columns (54..56) are cleared,
    /// which is how a slim texture always looks.
    fn slim_skin() -> Vec<u8> {
        let mut image = image::RgbaImage::new(64, 64);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([255, 0, 0, 255]);
        }
        for y in 20..32 {
            for x in 54..56 {
                image.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
            }
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn detects_slim_from_the_texture_not_the_metadata() {
        // Declared classic, but laid out slim — the texture wins, which is
        // what stops the arms rendering full of holes.
        let skin = Skin::decode(&slim_skin(), SkinModel::Classic).unwrap();
        assert_eq!(skin.detect_model(), SkinModel::Slim);

        let classic = Skin::decode(&solid_skin(), SkinModel::Slim).unwrap();
        assert_eq!(classic.detect_model(), SkinModel::Classic);
    }

    #[test]
    fn use_detected_model_changes_what_gets_rendered() {
        let mut skin = Skin::decode(&slim_skin(), SkinModel::Classic).unwrap();
        assert_eq!(skin.body(1).width, 16, "classic body before detection");

        assert_eq!(skin.use_detected_model(), SkinModel::Slim);
        assert_eq!(skin.body(1).width, 14, "slim body after detection");
    }

    #[test]
    fn rejects_wrong_dimensions() {
        let mut image = image::RgbaImage::new(32, 32);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgba([0, 0, 0, 255]);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        assert!(Skin::decode(&png, SkinModel::Classic).is_err());
    }

    #[test]
    fn cape_panel_crops_to_the_visible_face() {
        let mut texture = Rgba::new(64, 32);
        // Mark the panel's top-left so the crop origin can be checked.
        texture.set(1, 1, [1, 2, 3, 255]);
        // And something outside it, which must not survive.
        texture.set(40, 20, [9, 9, 9, 255]);

        let panel = cape_panel(&texture, 1);
        assert_eq!((panel.width, panel.height), (10, 16));
        assert_eq!(panel.get(0, 0), [1, 2, 3, 255]);

        let scaled = cape_panel(&texture, 3);
        assert_eq!((scaled.width, scaled.height), (30, 48));
    }

    #[test]
    fn placeholder_matches_classic_body_geometry() {
        let placeholder = placeholder_body(1);
        assert_eq!((placeholder.width, placeholder.height), (16, 32));
        // Corners sit outside the silhouette and must stay transparent.
        assert_eq!(placeholder.get(0, 0)[3], 0);
        // Centre of the torso is filled.
        assert_eq!(placeholder.get(8, 12)[3], 255);
    }

    #[test]
    fn blend_composites_semi_transparent_over_opaque() {
        let mut canvas = Rgba::new(1, 1);
        canvas.set(0, 0, [0, 0, 0, 255]);
        canvas.blend(0, 0, [255, 255, 255, 128]);
        let result = canvas.get(0, 0);
        assert_eq!(result[3], 255);
        // Halfway between black and white, allowing for rounding.
        assert!((result[0] as i32 - 128).abs() <= 2);
    }
}
