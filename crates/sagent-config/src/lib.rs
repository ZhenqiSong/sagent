pub mod paths;
pub mod profile;

pub use profile::{
    ProfileName, active_profile_path, list_profile_names, normalize_profile_name,
    read_active_profile, set_active_profile,
};
