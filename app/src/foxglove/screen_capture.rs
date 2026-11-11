//! Screen glow's capture (#496): DXGI Desktop Duplication through the `windows` crate, one
//! [`Capture`] per screen on the desktop.
//!
//! - `AcquireNextFrame` blocks until the screen changes (frame-driven, no timer). A frame where
//!   only the pointer moved is no new picture.
//! - The picture is copied into a texture with a mip chain on the GPU, `GenerateMips` shrinks it,
//!   and only one small level (at most [`MAX_WIDTH`] pixels wide, e.g. 240 x 135 from 4K) is
//!   copied to a staging texture and read back. The full-size picture never reaches the CPU.
//! - Errors (access lost on a mode change, a remote desktop, a driver reset) end the capture; the
//!   reader reports "capture failed" and opens a new one on the next display change.
//! - HDR desktops (#1085): the duplication asks for FP16 as well as 8-bit BGRA
//!   (`IDXGIOutput5::DuplicateOutput1`; Windows before 1703, or a refusal, falls back to plain
//!   `DuplicateOutput`). An FP16 picture is scRGB, linear light, so the GPU's mip average is an
//!   average of light; the small level is then tone-mapped to sRGB on the CPU ([`tone_map`],
//!   with the screen's SDR white level read once when it opens), with no shader.
//!
//! Under `cfg(test)` [`open`] always fails: tests never read the real screen.

use foxglove_scene::ScreenImage;

use super::screen_glow::Capture;

/// The widest picture read back: enough for the edge strips, small enough to cost nothing.
pub const MAX_WIDTH: u32 = 256;

/// The mip level that brings `width` to at most [`MAX_WIDTH`] (each level halves it).
pub fn level_for(width: u32) -> u32 {
    let mut level = 0;
    while (width >> level) > MAX_WIDTH && level < 12 {
        level += 1;
    }
    level
}

/// A picture `w` x `h` as it shows on a screen turned by `rotation` quarter turns clockwise
/// (Windows' `DXGI_MODE_ROTATION`: 0 none, 1 = 90, 2 = 180, 3 = 270).
pub fn rotated(
    monitor: &str,
    w: usize,
    h: usize,
    pixels: Vec<[u8; 3]>,
    rotation: u8,
) -> ScreenImage {
    let at = |x: usize, y: usize| pixels[y * w + x];
    let (ow, oh, px): (usize, usize, Vec<[u8; 3]>) = match rotation % 4 {
        1 => (
            h,
            w,
            (0..w)
                .flat_map(|y| (0..h).map(move |x| (x, y)))
                .map(|(x, y)| at(y, h - 1 - x))
                .collect(),
        ),
        2 => (
            w,
            h,
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| at(w - 1 - x, h - 1 - y))
                .collect(),
        ),
        3 => (
            h,
            w,
            (0..w)
                .flat_map(|y| (0..h).map(move |x| (x, y)))
                .map(|(x, y)| at(w - 1 - y, x))
                .collect(),
        ),
        _ => (w, h, pixels.clone()),
    };
    ScreenImage {
        monitor: monitor.into(),
        width: ow,
        height: oh,
        pixels: px,
    }
}

/// An IEEE 754 half-precision float (an FP16 desktop's channel) as an `f32`.
pub fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    match exp {
        0 => sign * frac * 2f32.powi(-24),
        0x1f if frac == 0.0 => sign * f32::INFINITY,
        0x1f => f32::NAN,
        _ => sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// One pixel of an HDR desktop (scRGB: linear light on sRGB's primaries, 1.0 = 80 nits) as
/// sRGB bytes. `white` is Windows' SDR white level in the same units (its "SDR content
/// brightness": SDR white shows at `white`), so an SDR window on an HDR desktop reads exactly
/// as it would on an SDR one. A highlight brighter than SDR white is scaled down to it as a
/// whole (its brightest channel to full, the others with it), so it keeps its hue rather than
/// washing out to white: the lights can't go brighter than full anyway. Colours outside sRGB's
/// gamut (negative scRGB) and broken values clip to dark. An unknown white level counts as
/// 80 nits (1.0).
pub fn tone_map(px: [f32; 3], white: f32) -> [u8; 3] {
    let white = if white.is_finite() && white > 0.05 {
        white
    } else {
        1.0
    };
    let c = px.map(|v| {
        if v.is_nan() {
            0.0
        } else {
            (v / white).clamp(0.0, 1.0e6)
        }
    });
    let m = c[0].max(c[1]).max(c[2]);
    let c = if m > 1.0 { c.map(|v| v / m) } else { c };
    foxglove_scene::Rgb::from_linear(c).0
}

/// The screens that opened, each failure logged and left out (a VR headset, an indirect or
/// virtual display, DisplayLink, a hybrid-GPU output). An error only when none opened.
pub fn readable<T>(opened: Vec<(String, Result<T, String>)>) -> Result<Vec<T>, String> {
    let mut out = Vec::new();
    let mut why = Vec::new();
    for (name, r) in opened {
        match r {
            Ok(s) => out.push(s),
            Err(e) => {
                eprintln!("Foxglove: screen glow leaves out {name}: it can't be read ({e})");
                why.push(format!("{name}: {e}"));
            }
        }
    }
    if out.is_empty() && !why.is_empty() {
        return Err(why.join("; "));
    }
    Ok(out)
}

/// One capture per screen on the desktop.
#[cfg(test)]
pub fn open() -> Result<Vec<Box<dyn Capture>>, String> {
    Err("tests never read the screen".into())
}

#[cfg(all(not(windows), not(test)))]
pub fn open() -> Result<Vec<Box<dyn Capture>>, String> {
    Err("Screen glow reads the screen on Windows only".into())
}

#[cfg(all(windows, not(test)))]
pub use imp::open;

#[cfg(all(windows, not(test)))]
mod imp {
    use std::time::Duration;

    use windows::core::Interface;
    use windows::Win32::Devices::Display::*;
    use windows::Win32::Foundation::{ERROR_SUCCESS, HMODULE};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;

    use super::super::screen_glow::Got;
    use super::{half, level_for, readable, rotated, tone_map, Capture, ScreenImage};

    /// A screen's name and how opening it went.
    type Opened = (String, Result<Box<dyn Capture>, String>);

    struct Screen {
        name: String,
        device: ID3D11Device,
        ctx: ID3D11DeviceContext,
        dup: IDXGIOutputDuplication,
        rotation: u8,
        /// The screen's SDR white level (scRGB, 1.0 = 80 nits), for an HDR desktop's pictures.
        white: f32,
        /// The full-size copy with its mip chain, its view for `GenerateMips`, its size, the
        /// level read back and its format.
        mips: Option<(
            ID3D11Texture2D,
            ID3D11ShaderResourceView,
            u32,
            u32,
            u32,
            DXGI_FORMAT,
        )>,
        staging: Option<ID3D11Texture2D>,
    }

    /// The desktop formats Screen glow reads: an HDR desktop's FP16, else 8-bit BGRA.
    const FORMATS: [DXGI_FORMAT; 2] = [DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM];

    /// The SDR white level of the display Windows names `gdi` (`\\.\DISPLAY1`), in scRGB
    /// (1.0 = 80 nits): where SDR white shows on its HDR desktop. `None` when Windows doesn't
    /// say (the tone map then takes 80 nits).
    ///
    /// # Safety
    /// Plain display-config queries into buffers sized by `GetDisplayConfigBufferSizes`; each
    /// request packet is a complete struct whose header names its own size.
    unsafe fn sdr_white(gdi: &str) -> Option<f32> {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return None;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        paths.truncate(np as usize);
        for p in &paths {
            let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: p.sourceInfo.adapterId,
                    id: p.sourceInfo.id,
                },
                viewGdiDeviceName: [0; 32],
            };
            if DisplayConfigGetDeviceInfo(&mut name.header) != 0 {
                continue;
            }
            let named = String::from_utf16_lossy(&name.viewGdiDeviceName);
            if named.trim_end_matches('\0') != gdi {
                continue;
            }
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                },
                SDRWhiteLevel: 0,
            };
            if DisplayConfigGetDeviceInfo(&mut white.header) != 0 || white.SDRWhiteLevel == 0 {
                return None;
            }
            return Some(white.SDRWhiteLevel as f32 / 1000.0);
        }
        None
    }

    // SAFETY: each Screen (its device, context and duplication) is used by one capture thread
    // only; it is made on the opening thread and moved there once.
    unsafe impl Send for Screen {}

    pub fn open() -> Result<Vec<Box<dyn Capture>>, String> {
        // SAFETY: plain COM factory and enumeration calls; every out value is checked.
        unsafe {
            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| format!("no DXGI factory: {e}"))?;
            let mut opened: Vec<Opened> = Vec::new();
            let mut a = 0;
            while let Ok(adapter) = factory.EnumAdapters1(a) {
                a += 1;
                let mut o = 0;
                while let Ok(output) = adapter.EnumOutputs(o) {
                    o += 1;
                    let Ok(desc) = output.GetDesc() else { continue };
                    if !desc.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    let name = String::from_utf16_lossy(&desc.DeviceName)
                        .trim_end_matches('\0')
                        .to_string();
                    let s = screen(&adapter, &output, name.clone())
                        .map(|s| Box::new(s) as Box<dyn Capture>)
                        .map_err(|e| e.to_string());
                    opened.push((name, s));
                }
            }
            readable(opened)
        }
    }

    /// One screen's device and duplication.
    ///
    /// # Safety
    /// COM calls on interfaces the caller got from DXGI's enumeration; each out value is checked
    /// before use.
    unsafe fn screen(
        adapter: &IDXGIAdapter1,
        output: &IDXGIOutput,
        name: String,
    ) -> windows::core::Result<Screen> {
        let mut device = None;
        let mut ctx = None;
        D3D11CreateDevice(
            &adapter.cast::<IDXGIAdapter>()?,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
        let device = device.ok_or_else(windows::core::Error::empty)?;
        let ctx = ctx.ok_or_else(windows::core::Error::empty)?;
        // HDR desktops need DuplicateOutput1 with FP16 among the formats; anything that refuses
        // it (older Windows, a driver) still reads 8-bit desktops as before.
        let dup = match output
            .cast::<IDXGIOutput5>()
            .and_then(|o| o.DuplicateOutput1(&device, 0, &FORMATS))
        {
            Ok(dup) => dup,
            Err(e) => {
                eprintln!("Foxglove: screen glow reads {name} without HDR ({e})");
                output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?
            }
        };
        let white = sdr_white(&name).unwrap_or(1.0);
        let rotation = match dup.GetDesc().Rotation {
            DXGI_MODE_ROTATION_ROTATE90 => 1,
            DXGI_MODE_ROTATION_ROTATE180 => 2,
            DXGI_MODE_ROTATION_ROTATE270 => 3,
            _ => 0,
        };
        Ok(Screen {
            name,
            device,
            ctx,
            dup,
            rotation,
            white,
            mips: None,
            staging: None,
        })
    }

    impl Screen {
        /// The mip texture and staging texture for a picture of this size and format, made once.
        ///
        /// # Safety
        /// Called on this screen's capture thread only (the device isn't shared). The
        /// descriptions are complete structs built here; every created texture and view is
        /// checked before it is kept.
        unsafe fn targets(&mut self, d: &D3D11_TEXTURE2D_DESC) -> windows::core::Result<()> {
            if self
                .mips
                .as_ref()
                .is_some_and(|m| (m.2, m.3, m.5) == (d.Width, d.Height, d.Format))
            {
                return Ok(());
            }
            if d.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
                eprintln!(
                    "Foxglove: screen glow: {} is HDR (SDR white {:.2} x 80 nits)",
                    self.name, self.white
                );
            }
            let level = level_for(d.Width);
            let desc = D3D11_TEXTURE2D_DESC {
                Width: d.Width,
                Height: d.Height,
                MipLevels: level + 1,
                ArraySize: 1,
                Format: d.Format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
            };
            let mut mips = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut mips))?;
            let mips = mips.ok_or_else(windows::core::Error::empty)?;
            let mut view = None;
            self.device
                .CreateShaderResourceView(&mips, None, Some(&mut view))?;
            let view = view.ok_or_else(windows::core::Error::empty)?;
            let (sw, sh) = ((d.Width >> level).max(1), (d.Height >> level).max(1));
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: sw,
                Height: sh,
                MipLevels: 1,
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            let mut staging = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            self.staging = staging;
            self.mips = Some((mips, view, d.Width, d.Height, level, d.Format));
            Ok(())
        }

        /// Reads the small level back as sRGB pixels.
        ///
        /// # Safety
        /// Called on this screen's capture thread only, after `targets` made the staging texture
        /// for this size. The pixels are read only between a successful `Map` and its `Unmap`,
        /// and only when the mapped rows hold the level's width (`RowPitch >= width * bytes per
        /// pixel`, checked below): each row slice then lies inside the mapped memory
        /// (`RowPitch * height` bytes).
        unsafe fn read(&mut self) -> windows::core::Result<ScreenImage> {
            let (Some((mips, _, w, h, level, format)), Some(staging)) = (&self.mips, &self.staging)
            else {
                return Err(windows::core::Error::empty());
            };
            let hdr = *format == DXGI_FORMAT_R16G16B16A16_FLOAT;
            let bytes = if hdr { 8 } else { 4 };
            let (sw, sh) = ((w >> level).max(1) as usize, (h >> level).max(1) as usize);
            self.ctx
                .CopySubresourceRegion(staging, 0, 0, 0, 0, mips, *level, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            if mapped.pData.is_null() || (mapped.RowPitch as usize) < sw * bytes {
                self.ctx.Unmap(staging, 0);
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(0x887A_0004_u32 as i32),
                    "the read-back rows are shorter than the picture",
                ));
            }
            let mut pixels = Vec::with_capacity(sw * sh);
            for y in 0..sh {
                // SAFETY: inside the mapped memory (see above), read before `Unmap`.
                let row = std::slice::from_raw_parts(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    sw * bytes,
                );
                if hdr {
                    // RGBA halves, scRGB.
                    let white = self.white;
                    pixels.extend(row.as_chunks::<8>().0.iter().map(|p| {
                        let c = |i: usize| half(u16::from_le_bytes([p[i], p[i + 1]]));
                        tone_map([c(0), c(2), c(4)], white)
                    }));
                } else {
                    // An 8-bit desktop is BGRA.
                    pixels.extend(row.as_chunks::<4>().0.iter().map(|p| [p[2], p[1], p[0]]));
                }
            }
            self.ctx.Unmap(staging, 0);
            Ok(rotated(&self.name, sw, sh, pixels, self.rotation))
        }
    }

    impl Capture for Screen {
        fn next(&mut self, wait: Duration) -> Result<Got, String> {
            let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource = None;
            // SAFETY: the duplication, device and context are this thread's; each acquired frame
            // is released below before anything else is done with the duplication, and the
            // desktop texture is used only while the frame is held.
            unsafe {
                match self.dup.AcquireNextFrame(ms, &mut info, &mut resource) {
                    Ok(()) => {}
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Got::Unchanged),
                    Err(e) => return Err(format!("{}: {e}", self.name)),
                }
                let shown = (|| -> windows::core::Result<bool> {
                    // Only the pointer moved: no new picture.
                    if info.LastPresentTime == 0 {
                        return Ok(false);
                    }
                    let texture: ID3D11Texture2D = resource
                        .as_ref()
                        .ok_or_else(windows::core::Error::empty)?
                        .cast()?;
                    let mut d = D3D11_TEXTURE2D_DESC::default();
                    texture.GetDesc(&mut d);
                    if !FORMATS.contains(&d.Format) {
                        return Err(windows::core::Error::new(
                            windows::core::HRESULT(0x887A_0004_u32 as i32),
                            "the desktop is neither 8-bit BGRA nor FP16",
                        ));
                    }
                    self.targets(&d)?;
                    if let Some((mips, view, ..)) = &self.mips {
                        self.ctx
                            .CopySubresourceRegion(mips, 0, 0, 0, 0, &texture, 0, None);
                        self.ctx.GenerateMips(view);
                    }
                    Ok(true)
                })();
                // The frame goes back to Windows before the read, which waits on the GPU.
                let _ = self.dup.ReleaseFrame();
                match shown {
                    Ok(false) => Ok(Got::Pointer),
                    Ok(true) => self
                        .read()
                        .map(Got::Picture)
                        .map_err(|e| format!("{}: {e}", self.name)),
                    Err(e) => Err(format!("{}: {e}", self.name)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_downscale_keeps_at_most_256_pixels_across() {
        assert_eq!(level_for(3840), 4, "4K: 240 across");
        assert_eq!(3840 >> level_for(3840), 240);
        assert_eq!(1920 >> level_for(1920), 240);
        assert_eq!(2560 >> level_for(2560), 160);
        assert_eq!(level_for(200), 0);
    }

    #[test]
    fn a_turned_screen_reads_upright() {
        // 3 x 2: a red pixel at the panel's top left.
        let mut px = vec![[0, 0, 0]; 6];
        px[0] = [255, 0, 0];
        let r = rotated("d", 3, 2, px.clone(), 1);
        assert_eq!((r.width, r.height), (2, 3));
        let r2 = rotated("d", 3, 2, px.clone(), 2);
        assert_eq!(r2.pixels[5], [255, 0, 0], "180: bottom right");
        let r0 = rotated("d", 3, 2, px, 0);
        assert_eq!(r0.pixels[0], [255, 0, 0]);
    }

    /// #496 review: one screen that can't be read (VR, a virtual display, DisplayLink) is left
    /// out; only none at all fails.
    #[test]
    fn an_unreadable_screen_is_left_out() {
        let r = readable(vec![
            ("DISPLAY1".to_string(), Ok(1)),
            ("VR".to_string(), Err("unsupported".to_string())),
            ("DISPLAY2".to_string(), Ok(2)),
        ]);
        assert_eq!(r, Ok(vec![1, 2]));
        let none: Result<Vec<u8>, String> =
            readable(vec![("VR".into(), Err("unsupported".into()))]);
        assert!(none.unwrap_err().contains("VR: unsupported"));
        assert_eq!(readable::<u8>(Vec::new()), Ok(Vec::new()));
    }

    /// #1085: FP16 halves as an HDR desktop gives them.
    #[test]
    fn halves_decode() {
        assert_eq!(half(0x0000), 0.0);
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0x3800), 0.5);
        assert_eq!(half(0x4000), 2.0);
        assert_eq!(half(0x4900), 10.0);
        assert_eq!(half(0xbc00), -1.0);
        assert_eq!(half(0x7bff), 65504.0, "the largest");
        assert_eq!(half(0x0001), 2f32.powi(-24), "the smallest subnormal");
        assert_eq!(half(0x7c00), f32::INFINITY);
        assert!(half(0x7e00).is_nan());
    }

    /// #1085: the tone map on synthetic scRGB pixels. SDR content on an HDR desktop reads as it
    /// would on an SDR one, highlights come down to full brightness at their own hue, and
    /// out-of-gamut or broken values never light up.
    #[test]
    fn the_tone_map_brings_hdr_down_to_srgb() {
        let white = 2.5; // "SDR content brightness" at 200 nits
        assert_eq!(
            tone_map([white; 3], white),
            [255, 255, 255],
            "SDR white is white"
        );
        assert_eq!(tone_map([0.0; 3], white), [0, 0, 0]);
        // Every SDR colour comes through as itself (Bandit orange, a mid grey).
        for srgb in [[0xf0, 0x8a, 0x3c], [128, 128, 128], [20, 40, 230]] {
            let scrgb = foxglove_scene::Rgb(srgb).linear().map(|v| v * white);
            assert_eq!(tone_map(scrgb, white), srgb);
        }
        // Highlights: full brightness, the hue kept (an orange glare stays orange).
        let glare = tone_map([40.0 * white, 20.0 * white, 0.0], white);
        assert_eq!(glare, foxglove_scene::Rgb::from_linear([1.0, 0.5, 0.0]).0);
        assert_eq!(tone_map([4.0 * white; 3], white), [255; 3]);
        let ramp: Vec<u8> = (0..40)
            .map(|i| tone_map([i as f32 * 0.1 * white; 3], white)[0])
            .collect();
        assert!(ramp.windows(2).all(|w| w[1] >= w[0]), "{ramp:?}");
        // Wide-gamut (negative scRGB) and broken values.
        assert_eq!(tone_map([-0.5, 0.0, 0.0], white), [0, 0, 0]);
        assert_eq!(tone_map([f32::NAN, f32::INFINITY, 0.0], white), [0, 255, 0]);
        // An unknown white level reads as 80 nits.
        assert_eq!(tone_map([0.5; 3], 0.0), tone_map([0.5; 3], 1.0));
        // The same picture at two SDR brightness settings reads the same.
        let at = |w: f32| tone_map([0.3 * w, 0.6 * w, 0.1 * w], w);
        assert_eq!(at(1.0), at(5.0));
    }

    #[test]
    fn tests_never_read_the_screen() {
        assert!(open().is_err());
    }
}
