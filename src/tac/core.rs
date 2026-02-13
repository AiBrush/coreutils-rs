use std::io::{self, IoSlice, Write};

/// Max IoSlice entries per write_vectored batch.
/// Linux UIO_MAXIOV is 1024; we use that as our batch limit.
const MAX_IOV: usize = 1024;

/// Chunk size for the forward-scan-within-backward-chunks strategy.
/// 2MB gives better SIMD throughput per memchr_iter call (fewer calls needed,
/// each processing a larger contiguous region with full AVX2 pipeline).
/// For a 10MB file with 50-byte lines, 2MB chunks = 5 calls vs 20 at 512KB.
const CHUNK: usize = 2 * 1024 * 1024;

/// Flush a batch of IoSlice entries using write_vectored.
/// Falls back to individual write_all for each slice if write_vectored
/// doesn't write everything (handles partial writes).
#[inline]
fn flush_iov(out: &mut impl Write, slices: &[IoSlice]) -> io::Result<()> {
    if slices.is_empty() {
        return Ok(());
    }
    // Try write_vectored first for the whole batch
    let total: usize = slices.iter().map(|s| s.len()).sum();

    // Fast path: single writev call often writes everything
    let written = match out.write_vectored(slices) {
        Ok(n) if n >= total => return Ok(()),
        Ok(n) => n,
        Err(e) => return Err(e),
    };

    // Slow path: partial write — fall back to write_all per remaining slice
    let mut skip = written;
    for slice in slices {
        let slen = slice.len();
        if skip >= slen {
            skip -= slen;
            continue;
        }
        if skip > 0 {
            out.write_all(&slice[skip..])?;
            skip = 0;
        } else {
            out.write_all(slice)?;
        }
    }
    Ok(())
}

/// Reverse records separated by a single byte.
/// Uses chunk-based forward SIMD scan processed in reverse order for
/// maximum throughput — eliminates per-line memrchr call overhead.
/// Output uses write_vectored (writev) for zero-copy from mmap'd data.
pub fn tac_bytes(data: &[u8], separator: u8, before: bool, out: &mut impl Write) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    if !before {
        tac_bytes_backward_after(data, separator, out)
    } else {
        tac_bytes_backward_before(data, separator, out)
    }
}

/// After-separator mode: chunk-based forward SIMD scan, emitted in reverse.
///
/// Instead of calling memrchr once per line (high per-call overhead for short
/// lines), we process the file backward in large chunks. Within each chunk a
/// single memchr_iter forward pass finds ALL separator positions with full SIMD
/// pipeline utilisation, then we emit the records (slices) in reverse order.
///
/// This converts ~200K memrchr calls (for a 10MB / 50-byte-line file) into
/// ~20 memchr_iter calls, each scanning a contiguous 512KB region.
///
/// Optimization: Instead of collecting positions into a Vec, we store them
/// in a stack-allocated array to avoid heap allocation per chunk.
fn tac_bytes_backward_after(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);

    // `global_end` tracks where the current (rightmost unseen) record ends.
    let mut global_end = data.len();

    // Stack-allocated positions buffer: avoid heap allocation per chunk.
    // 512KB chunk / 1 byte per separator = 512K max positions.
    // We use a heap-allocated buffer once but reuse across all chunks.
    let mut positions_buf: Vec<usize> = Vec::with_capacity(CHUNK);

    let mut chunk_start = data.len().saturating_sub(CHUNK);

    loop {
        let chunk_end = global_end.min(data.len());
        if chunk_start >= chunk_end {
            if chunk_start == 0 {
                break;
            }
            chunk_start = chunk_start.saturating_sub(CHUNK);
            continue;
        }
        let chunk = &data[chunk_start..chunk_end];

        // Reuse positions buffer: clear and refill without reallocation.
        positions_buf.clear();
        positions_buf.extend(memchr::memchr_iter(sep, chunk).map(|p| p + chunk_start));

        // Emit records in reverse (rightmost first).
        for &pos in positions_buf.iter().rev() {
            if pos + 1 < global_end {
                iov.push(IoSlice::new(&data[pos + 1..global_end]));
            }
            global_end = pos + 1;

            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }

        if chunk_start == 0 {
            break;
        }
        chunk_start = chunk_start.saturating_sub(CHUNK);
    }

    // First record
    if global_end > 0 {
        iov.push(IoSlice::new(&data[0..global_end]));
    }
    flush_iov(out, &iov)?;
    Ok(())
}

/// Before-separator mode: chunk-based forward SIMD scan, emitted in reverse.
///
/// Same chunked strategy as after-mode, but each record STARTS with its
/// separator byte instead of ending with it.
fn tac_bytes_backward_before(data: &[u8], sep: u8, out: &mut impl Write) -> io::Result<()> {
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);

    let mut global_end = data.len();
    let mut chunk_start = data.len().saturating_sub(CHUNK);
    let mut positions_buf: Vec<usize> = Vec::with_capacity(CHUNK);

    loop {
        let chunk_end = global_end.min(data.len());
        if chunk_start >= chunk_end {
            if chunk_start == 0 {
                break;
            }
            chunk_start = chunk_start.saturating_sub(CHUNK);
            continue;
        }
        let chunk = &data[chunk_start..chunk_end];

        positions_buf.clear();
        positions_buf.extend(memchr::memchr_iter(sep, chunk).map(|p| p + chunk_start));

        for &pos in positions_buf.iter().rev() {
            iov.push(IoSlice::new(&data[pos..global_end]));
            global_end = pos;

            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }

        if chunk_start == 0 {
            break;
        }
        chunk_start = chunk_start.saturating_sub(CHUNK);
    }

    if global_end > 0 {
        iov.push(IoSlice::new(&data[0..global_end]));
    }
    flush_iov(out, &iov)?;
    Ok(())
}

/// Reverse records using a multi-byte string separator.
/// Uses chunk-based forward SIMD-accelerated memmem + IoSlice zero-copy output.
///
/// For single-byte separators, delegates to tac_bytes which uses memchr (faster).
pub fn tac_string_separator(
    data: &[u8],
    separator: &[u8],
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    if separator.len() == 1 {
        return tac_bytes(data, separator[0], before, out);
    }

    let sep_len = separator.len();

    // For multi-byte separators we use the same chunk-based strategy but with
    // memmem::find_iter instead of memchr_iter. We need FinderRev only for a
    // quick "any separator exists?" check — the actual work uses forward Finder.
    //
    // We still need to handle the case where a separator straddles a chunk
    // boundary. We do this by extending each chunk's left edge by (sep_len - 1)
    // bytes and deduplicating matches that fall in the overlap zone.

    if !before {
        tac_string_after(data, separator, sep_len, out)
    } else {
        tac_string_before(data, separator, sep_len, out)
    }
}

/// Multi-byte string separator, after mode (separator at end of record).
fn tac_string_after(
    data: &[u8],
    separator: &[u8],
    sep_len: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut global_end = data.len();
    let mut chunk_start = data.len().saturating_sub(CHUNK);
    let mut positions_buf: Vec<usize> = Vec::with_capacity(CHUNK / 4);

    loop {
        let chunk_end = global_end.min(data.len());
        let scan_start = chunk_start.saturating_sub(sep_len - 1);
        if scan_start >= chunk_end {
            if chunk_start == 0 {
                break;
            }
            chunk_start = chunk_start.saturating_sub(CHUNK);
            continue;
        }
        let scan_region = &data[scan_start..chunk_end];

        positions_buf.clear();
        positions_buf.extend(
            memchr::memmem::find_iter(scan_region, separator)
                .map(|p| p + scan_start)
                .filter(|&p| p >= chunk_start || chunk_start == 0)
                .filter(|&p| p + sep_len <= global_end),
        );

        for &pos in positions_buf.iter().rev() {
            let rec_end_exclusive = pos + sep_len;
            if rec_end_exclusive < global_end {
                iov.push(IoSlice::new(&data[rec_end_exclusive..global_end]));
            }
            global_end = rec_end_exclusive;
            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }

        if chunk_start == 0 {
            break;
        }
        chunk_start = chunk_start.saturating_sub(CHUNK);
    }

    if global_end > 0 {
        iov.push(IoSlice::new(&data[0..global_end]));
    }
    flush_iov(out, &iov)?;
    Ok(())
}

/// Multi-byte string separator, before mode (separator at start of record).
fn tac_string_before(
    data: &[u8],
    separator: &[u8],
    sep_len: usize,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut iov: Vec<IoSlice> = Vec::with_capacity(MAX_IOV);
    let mut global_end = data.len();
    let mut chunk_start = data.len().saturating_sub(CHUNK);
    let mut positions_buf: Vec<usize> = Vec::with_capacity(CHUNK / 4);

    loop {
        let chunk_end = global_end.min(data.len());
        let scan_start = chunk_start.saturating_sub(sep_len - 1);
        if scan_start >= chunk_end {
            if chunk_start == 0 {
                break;
            }
            chunk_start = chunk_start.saturating_sub(CHUNK);
            continue;
        }
        let scan_region = &data[scan_start..chunk_end];

        positions_buf.clear();
        positions_buf.extend(
            memchr::memmem::find_iter(scan_region, separator)
                .map(|p| p + scan_start)
                .filter(|&p| p >= chunk_start || chunk_start == 0)
                .filter(|&p| p < global_end),
        );

        for &pos in positions_buf.iter().rev() {
            iov.push(IoSlice::new(&data[pos..global_end]));
            global_end = pos;
            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }

        if chunk_start == 0 {
            break;
        }
        chunk_start = chunk_start.saturating_sub(CHUNK);
    }

    if global_end > 0 {
        iov.push(IoSlice::new(&data[0..global_end]));
    }
    flush_iov(out, &iov)?;
    Ok(())
}

/// Find regex matches using backward scanning, matching GNU tac's re_search behavior.
fn find_regex_matches_backward(data: &[u8], re: &regex::bytes::Regex) -> Vec<(usize, usize)> {
    let mut matches = Vec::new();
    let mut past_end = data.len();

    while past_end > 0 {
        let buf = &data[..past_end];
        let mut found = false;

        let mut pos = past_end;
        while pos > 0 {
            pos -= 1;
            if let Some(m) = re.find_at(buf, pos) {
                if m.start() == pos {
                    matches.push((m.start(), m.end()));
                    past_end = m.start();
                    found = true;
                    break;
                }
            }
        }

        if !found {
            break;
        }
    }

    matches.reverse();
    matches
}

/// Reverse records using a regex separator.
pub fn tac_regex_separator(
    data: &[u8],
    pattern: &str,
    before: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }

    let re = match regex::bytes::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid regex '{}': {}", pattern, e),
            ));
        }
    };

    let matches = find_regex_matches_backward(data, &re);

    if matches.is_empty() {
        out.write_all(data)?;
        return Ok(());
    }

    let mut iov: Vec<IoSlice> = Vec::with_capacity(matches.len().min(MAX_IOV) + 2);

    if !before {
        let last_end = matches.last().unwrap().1;

        if last_end < data.len() {
            iov.push(IoSlice::new(&data[last_end..]));
        }

        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let rec_start = if i == 0 { 0 } else { matches[i - 1].1 };
            iov.push(IoSlice::new(&data[rec_start..matches[i].1]));
            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }
    } else {
        let mut i = matches.len();
        while i > 0 {
            i -= 1;
            let start = matches[i].0;
            let end = if i + 1 < matches.len() {
                matches[i + 1].0
            } else {
                data.len()
            };
            iov.push(IoSlice::new(&data[start..end]));
            if iov.len() >= MAX_IOV {
                flush_iov(out, &iov)?;
                iov.clear();
            }
        }

        if matches[0].0 > 0 {
            iov.push(IoSlice::new(&data[..matches[0].0]));
        }
    }

    flush_iov(out, &iov)?;
    Ok(())
}
