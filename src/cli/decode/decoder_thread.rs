use super::processor::{ProcessFramesContext, process_frames};
use crate::input::InputReader;
use anyhow::Result;
use indicatif::ProgressBar;
use std::sync::mpsc;
use std::thread;
use truehd::process::{decode::Decoder, extract::Extractor, parse::Parser};

pub struct DecoderThreadConfig {
    pub input_path: std::path::PathBuf,
    pub presentation: u8,
    pub strict_mode: bool,
    pub tx: mpsc::Sender<Result<truehd::process::decode::DecodedAccessUnit>>,
    pub pb_clone: Option<ProgressBar>,
    pub extractor: Extractor,
    pub parser: Parser,
    pub decoder: Decoder,
}

pub fn spawn_decoder_thread(config: DecoderThreadConfig) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || -> Result<()> {
        let DecoderThreadConfig {
            input_path,
            presentation,
            strict_mode,
            tx,
            pb_clone,
            mut extractor,
            mut parser,
            mut decoder,
        } = config;

        let mut frame_count: u64 = 0;
        let mut total_samples = 0u64;
        let mut frames_processed = 0;
        let mut current_substream_info: Option<u8> = None;
        let mut current_extended_substream_info: Option<u8> = None;

        let mut input_reader = InputReader::new(&input_path)?;

        input_reader.process_chunks(64 * 1024, |chunk| {
            extractor.push_bytes(chunk);

            let mut ctx = ProcessFramesContext {
                extractor: &mut extractor,
                parser: &mut parser,
                decoder: &mut decoder,
                frames_processed: &mut frames_processed,
                frame_count: &mut frame_count,
                total_samples: &mut total_samples,
                presentation,
                strict_mode,
                tx: &tx,
                pb_clone: &pb_clone,
                current_substream_info: &mut current_substream_info,
                current_extended_substream_info: &mut current_extended_substream_info,
            };

            let should_exit = process_frames(&mut ctx)?;

            Ok(!should_exit) // Convert exit signal to continue signal
        })?;

        log::info!("Processing complete: {frame_count} frames, {total_samples} samples");
        Ok(())
    })
}
