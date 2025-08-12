use super::output::create_path_with_suffix;
use crate::damf::Data;
use anyhow::Result;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

fn write_damf_header_to_file(header_path: &Path, damf_data: &Data) -> Result<()> {
    log::info!("Creating DAMF header file: {}", header_path.display());
    let mut header_writer = BufWriter::new(File::create(header_path)?);
    let header_str = &damf_data.serialize_damf();
    write!(header_writer, "{header_str}")?;
    header_writer.flush()?;
    Ok(())
}

pub fn create_damf_header_file(
    base_path: &Path,
    oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
    warp_mode: Option<crate::cli::command::WarpMode>,
) -> Result<()> {
    let header_path = create_path_with_suffix(base_path, "atmos");
    let mut damf_data = Data::with_oamd_payload(oamd, base_path);

    // Override warp_mode if specified and not present in metadata
    if let Some(cli_warp_mode) = warp_mode {
        if let Some(presentation) = damf_data.presentations_mut().first_mut() {
            if presentation.warp_mode.is_none() {
                presentation.warp_mode = Some(cli_warp_mode.into());
            }
        }
    }

    write_damf_header_to_file(&header_path, &damf_data)
}

pub fn create_atmos_header_path(base_path: &Path) -> PathBuf {
    create_path_with_suffix(base_path, "atmos")
}

pub fn rewrite_damf_header_for_bed_conform(
    base_path: &Path,
    oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
    warp_mode: Option<crate::cli::command::WarpMode>,
) -> Result<()> {
    let header_path = create_atmos_header_path(base_path);
    let mut damf_data = Data::with_oamd_payload_bed_conform(oamd, base_path);

    // Override warp_mode if specified and not present in metadata
    if let Some(cli_warp_mode) = warp_mode {
        if let Some(presentation) = damf_data.presentations_mut().first_mut() {
            if presentation.warp_mode.is_none() {
                presentation.warp_mode = Some(cli_warp_mode.into());
            }
        }
    }

    write_damf_header_to_file(&header_path, &damf_data)
}
