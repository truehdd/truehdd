use crate::damf::Data;
use anyhow::Result;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub fn create_damf_header_file(
    base_path: &Path,
    oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
) -> Result<()> {
    let header_path = {
        let mut path = base_path.to_path_buf();
        let new_name = format!("{}.atmos", base_path.file_name().unwrap().to_string_lossy());
        path.set_file_name(new_name);
        path
    };

    log::info!("Creating DAMF header file: {}", header_path.display());
    let mut header_writer = BufWriter::new(File::create(header_path)?);

    let damf_data = Data::with_oamd_payload(oamd, base_path);
    let header_str = &damf_data.serialize_damf();
    write!(header_writer, "{header_str}")?;
    header_writer.flush()?;

    Ok(())
}

pub fn create_atmos_header_path(base_path: &Path) -> PathBuf {
    let mut path = base_path.to_path_buf();
    let new_name = format!("{}.atmos", base_path.file_name().unwrap().to_string_lossy());
    path.set_file_name(new_name);
    path
}
