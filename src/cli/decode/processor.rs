use anyhow::Result;
use indicatif::ProgressBar;
use std::sync::mpsc;
use truehd::process::{decode::Decoder, extract::Extractor, parse::Parser};

pub struct ProcessFramesContext<'a> {
    pub extractor: &'a mut Extractor,
    pub parser: &'a mut Parser,
    pub decoder: &'a mut Decoder,
    pub frames_processed: &'a mut u64,
    pub frame_count: &'a mut u64,
    pub total_samples: &'a mut u64,
    pub presentation: u8,
    pub strict_mode: bool,
    pub tx: &'a mpsc::Sender<Result<truehd::process::decode::DecodedAccessUnit>>,
    pub pb_clone: &'a Option<ProgressBar>,
}

pub fn process_frames(ctx: &mut ProcessFramesContext) -> Result<bool> {
    loop {
        match ctx.extractor.next() {
            Some(Ok(frame)) => {
                *ctx.frames_processed += 1;
                if let Some(pb) = ctx.pb_clone {
                    pb.set_position(*ctx.frames_processed);
                }
                *ctx.frame_count += 1;

                match ctx.parser.parse(&frame) {
                    Ok(access_unit) => {
                        match ctx
                            .decoder
                            .decode_presentation(&access_unit, ctx.presentation as usize)
                        {
                            Ok(decoded) => {
                                *ctx.total_samples += decoded.sample_length as u64;
                                if ctx.tx.send(Ok(decoded)).is_err() {
                                    return Ok(true);
                                }
                            }
                            Err(e) => {
                                log::error!("Decode error at frame {}: {e}", *ctx.frame_count);
                                if ctx.strict_mode {
                                    let _ = ctx.tx.send(Err(e));
                                    return Ok(true);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Parse error at frame {}: {e}", *ctx.frame_count);
                        if ctx.strict_mode {
                            let _ = ctx.tx.send(Err(e));
                            return Ok(true);
                        }
                    }
                }
            }
            Some(Err(ref e))
                if matches!(e, truehd::utils::errors::ExtractError::InsufficientData) =>
            {
                break;
            }
            Some(Err(_extract_error)) => {
                if let Some(pb) = ctx.pb_clone {
                    pb.set_message("processing (some extraction errors)");
                }
            }
            None => {
                break;
            }
        }
    }
    Ok(false)
}
