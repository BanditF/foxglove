//! Scenes (#481): one room map, one clock, and effects that travel through every light in it.
//! See the README for the units and the model.

pub mod burst;
pub mod clock;
pub mod colour;
pub mod effects;
pub mod room;
pub mod scheduler;
pub mod space;
pub mod wallpaper;

pub use burst::{Burst, Bursts};
pub use clock::{Clock, SceneTime};
pub use colour::Rgb;
pub use effects::{Effect, Frame};
pub use room::{DeviceInfo, DeviceKind, Layout, LightSpec, MonitorInfo, RoomMap, Side};
pub use space::{Point, Rect};
pub use wallpaper::WallpaperScene;
