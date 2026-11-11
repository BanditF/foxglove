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
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;

    use super::super::screen_glow::Got;
    use super::{level_for, readable, rotated, Capture, ScreenImage};

    /// A screen's name and how opening it went.
    type Opened = (String, Result<Box<dyn Capture>, String>);

    struct Screen {
        name: String,
        device: ID3D11Device,
        ctx: ID3D11DeviceContext,
        dup: IDXGIOutputDuplication,
        rotation: u8,
        /// The full-size copy with its mip chain, and its view for `GenerateMips`.
        mips: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, u32, u32, u32)>,
        staging: Option<ID3D11Texture2D>,
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
        let dup = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
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
                .is_some_and(|m| (m.2, m.3) == (d.Width, d.Height))
            {
                return Ok(());
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
            self.mips = Some((mips, view, d.Width, d.Height, level));
            Ok(())
        }

        /// Reads the small level back as sRGB pixels.
        ///
        /// # Safety
        /// Called on this screen's capture thread only, after `targets` made the staging texture
        /// for this size. The pixels are read only between a successful `Map` and its `Unmap`,
        /// and only when the mapped rows hold the level's width (`RowPitch >= width * 4`, checked
        /// below): each row slice then lies inside the mapped memory (`RowPitch * height` bytes).
        unsafe fn read(&mut self) -> windows::core::Result<ScreenImage> {
            let (Some((mips, _, w, h, level)), Some(staging)) = (&self.mips, &self.staging) else {
                return Err(windows::core::Error::empty());
            };
            let (sw, sh) = ((w >> level).max(1) as usize, (h >> level).max(1) as usize);
            self.ctx
                .CopySubresourceRegion(staging, 0, 0, 0, 0, mips, *level, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            if mapped.pData.is_null() || (mapped.RowPitch as usize) < sw * 4 {
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
                    sw * 4,
                );
                // The desktop is BGRA.
                pixels.extend(row.as_chunks::<4>().0.iter().map(|p| [p[2], p[1], p[0]]));
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
                    if d.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                        return Err(windows::core::Error::new(
                            windows::core::HRESULT(0x887A_0004_u32 as i32),
                            "the desktop isn't 8-bit BGRA",
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

    #[test]
    fn tests_never_read_the_screen() {
        assert!(open().is_err());
    }
}
