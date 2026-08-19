//! One module per overlay theme. Each owns its tunable consts, its per-frame state (if any),
//! and its painter; [`crate::render_frame`] dispatches on [`crate::OverlayStyle`].

pub(crate) mod bars;
pub(crate) mod hud;
pub(crate) mod volt;
pub(crate) mod wave;
