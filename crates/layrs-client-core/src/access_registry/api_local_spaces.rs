mod draft_linking;
mod layers;
mod local;

pub(crate) use draft_linking::link_active_local_layer_for_sync;
pub use draft_linking::send_draft_local_space;
pub use layers::{
    clear_layer_steps, create_layer_from_current, delete_layer, disconnect_layer_from_parent,
    switch_layer, weave_active_layer_to_parent,
};
#[cfg(test)]
pub(crate) use local::create_local_space_internal;
pub use local::{
    create_draft_local_space, create_local_space, forget_local_space, init_local_space,
    list_available_spaces, list_local_spaces, open_local_space,
};
