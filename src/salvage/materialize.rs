//! Read by extent, and materialize from a plan.

use super::{ExtentWalk, PieceKind, Salvage};
use crate::error::{Error, Result};
use crate::extent::DataExtent;
use fs_core::BlockRead;
use std::io;

/// Bytes copied per source read.
const CHUNK: u64 = 1 << 20;

/// One step of a plan: where the next bytes of the output come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStep {
    /// `length` bytes of the device from byte `offset`, as stored.
    Source { offset: u64, length: u64 },
    /// `length` bytes from `skip` bytes into a stored extent's live data,
    /// read with its checksum checked and decompressed.
    Extent {
        extent: DataExtent,
        skip: u64,
        length: u64,
    },
    /// Zeros: a hole, a reservation, an unwritten extent.
    Zeros { length: u64 },
    /// Bytes given in the plan, such as inline data.
    Literal(Vec<u8>),
    /// Bytes nothing can supply: lost, not followed or undecodable. Written
    /// as zeros and reported as missing, never as data.
    Missing { length: u64, why: String },
}

impl PlanStep {
    pub fn length(&self) -> u64 {
        match self {
            PlanStep::Source { length, .. }
            | PlanStep::Extent { length, .. }
            | PlanStep::Zeros { length }
            | PlanStep::Missing { length, .. } => *length,
            PlanStep::Literal(b) => b.len() as u64,
        }
    }
}

/// The steps that make an output, in order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
}

impl Plan {
    pub fn length(&self) -> u64 {
        self.steps.iter().map(PlanStep::length).sum()
    }
}

/// Where a range of the output came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeSource {
    /// The device, from this byte.
    Source(u64),
    /// A stored extent's decoded data, stored at this device byte.
    Extent(u64),
    Zeros,
    Literal,
    /// Nothing supplied it: zeros were written in its place.
    Missing(String),
}

/// One range of the output and its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedRange {
    pub output_offset: u64,
    pub length: u64,
    pub source: RangeSource,
}

/// What a materialization wrote, byte range by byte range.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaterializeReport {
    pub written: u64,
    pub ranges: Vec<MaterializedRange>,
}

impl MaterializeReport {
    /// Output ranges no source supplied.
    pub fn missing(&self) -> impl Iterator<Item = &MaterializedRange> {
        self.ranges
            .iter()
            .filter(|r| matches!(r.source, RangeSource::Missing(_)))
    }

    /// Whether every byte came from a source, a literal or a genuine hole.
    pub fn is_complete(&self) -> bool {
        self.missing().next().is_none()
    }

    fn push(&mut self, length: u64, source: RangeSource) {
        if length == 0 {
            return;
        }
        self.ranges.push(MaterializedRange {
            output_offset: self.written,
            length,
            source,
        });
        self.written += length;
    }
}

impl Salvage {
    /// `output.len()` bytes of the device from byte `offset`. Refused when
    /// the range runs past the device.
    pub fn read_extent(&self, dev: &dyn BlockRead, offset: u64, output: &mut [u8]) -> Result<()> {
        let end = offset
            .checked_add(output.len() as u64)
            .filter(|&e| e <= dev.size_bytes())
            .ok_or_else(|| {
                Error::Corrupt(format!(
                    "{} bytes at byte {offset} run past the device ({} bytes)",
                    output.len(),
                    dev.size_bytes()
                ))
            })?;
        let _ = end;
        dev.read_at(offset, output)?;
        Ok(())
    }

    /// A plan that makes a walked inode's contents: its pieces in file
    /// order, its holes as zeros, everything clipped to its size. Data
    /// stored plain is read straight from the device; checksummed or
    /// compressed data is read through its extent.
    pub fn plan(&self, walk: &ExtentWalk) -> Plan {
        let size = walk.size;
        let mut steps = Vec::new();
        let mut at = 0u64;
        for p in &walk.pieces {
            if p.file_offset >= size {
                break;
            }
            if p.file_offset > at {
                steps.push(PlanStep::Zeros {
                    length: p.file_offset - at,
                });
            }
            let length = p.length.min(size - p.file_offset);
            steps.push(match &p.kind {
                PieceKind::Data {
                    unwritten: true, ..
                } => PlanStep::Zeros { length },
                PieceKind::Data {
                    extent, data_skip, ..
                } => {
                    let skip = *data_skip;
                    match extent.crc {
                        None => PlanStep::Source {
                            offset: extent.ptr.offset.saturating_mul(512) + skip,
                            length,
                        },
                        Some(_) => PlanStep::Extent {
                            extent: extent.clone(),
                            skip,
                            length,
                        },
                    }
                }
                PieceKind::Inline(bytes) => {
                    let mut b = bytes.clone();
                    b.resize(length as usize, 0);
                    PlanStep::Literal(b)
                }
                PieceKind::Zeros => PlanStep::Zeros { length },
                PieceKind::Lost => PlanStep::Missing {
                    length,
                    why: "the filesystem recorded this data as lost".into(),
                },
                PieceKind::Reflink { index } => PlanStep::Missing {
                    length,
                    why: format!("reflinked data at index {index} is not followed"),
                },
                PieceKind::Undecodable(why) => PlanStep::Missing {
                    length,
                    why: why.clone(),
                },
            });
            at = p.file_offset + length;
        }
        if at < size {
            steps.push(PlanStep::Zeros { length: size - at });
        }
        Plan { steps }
    }

    /// Write the plan's output to `sink`, never to the source, and say where
    /// every byte came from. A source range that does not read, or an extent
    /// that does not decode or fails its checksum, is written as zeros and
    /// reported missing; only a failure to write to `sink` is an error.
    pub fn materialize(
        &self,
        dev: &dyn BlockRead,
        plan: &Plan,
        sink: &mut dyn io::Write,
    ) -> io::Result<MaterializeReport> {
        let mut report = MaterializeReport::default();
        for step in &plan.steps {
            match step {
                PlanStep::Source { offset, length } => {
                    let mut done = 0u64;
                    while done < *length {
                        let n = (length - done).min(CHUNK);
                        let mut buf = vec![0u8; n as usize];
                        let at = offset + done;
                        match self.read_extent(dev, at, &mut buf) {
                            Ok(()) => {
                                sink.write_all(&buf)?;
                                report.push(n, RangeSource::Source(at));
                            }
                            Err(e) => {
                                sink.write_all(&vec![0u8; n as usize])?;
                                report.push(n, RangeSource::Missing(e.to_string()));
                            }
                        }
                        done += n;
                    }
                }
                PlanStep::Extent {
                    extent,
                    skip,
                    length,
                } => {
                    let decoded =
                        crate::fs::extent_bytes(extent, None, dev.size_bytes(), |at, buf| {
                            Ok(dev.read_at(at, buf)?)
                        });
                    let window = decoded.as_ref().ok().and_then(|d| {
                        let s = usize::try_from(*skip).ok()?;
                        let e = s.checked_add(usize::try_from(*length).ok()?)?;
                        d.get(s..e)
                    });
                    match (window, &decoded) {
                        (Some(w), _) => {
                            sink.write_all(w)?;
                            report.push(
                                *length,
                                RangeSource::Extent(extent.ptr.offset.saturating_mul(512)),
                            );
                        }
                        (None, result) => {
                            write_zeros(sink, *length)?;
                            let why = match result {
                                Err(e) => e.to_string(),
                                Ok(d) => format!(
                                    "the extent decodes to {} bytes, short of {} at {skip}",
                                    d.len(),
                                    length
                                ),
                            };
                            report.push(*length, RangeSource::Missing(why));
                        }
                    }
                }
                PlanStep::Zeros { length } => {
                    write_zeros(sink, *length)?;
                    report.push(*length, RangeSource::Zeros);
                }
                PlanStep::Literal(bytes) => {
                    sink.write_all(bytes)?;
                    report.push(bytes.len() as u64, RangeSource::Literal);
                }
                PlanStep::Missing { length, why } => {
                    write_zeros(sink, *length)?;
                    report.push(*length, RangeSource::Missing(why.clone()));
                }
            }
        }
        Ok(report)
    }
}

fn write_zeros(sink: &mut dyn io::Write, length: u64) -> io::Result<()> {
    let zeros = vec![0u8; length.min(CHUNK) as usize];
    let mut left = length;
    while left > 0 {
        let n = left.min(CHUNK);
        sink.write_all(&zeros[..n as usize])?;
        left -= n;
    }
    Ok(())
}
