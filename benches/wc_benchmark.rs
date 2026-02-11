use coreutils_rs::wc;
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

fn generate_text(lines: usize, words_per_line: usize) -> Vec<u8> {
    let mut data = Vec::new();
    for _ in 0..lines {
        for j in 0..words_per_line {
            if j > 0 {
                data.push(b' ');
            }
            data.extend_from_slice(b"hello");
        }
        data.push(b'\n');
    }
    data
}

fn bench_count_lines(c: &mut Criterion) {
    let mut group = c.benchmark_group("wc_lines");
    for size_mb in [1, 10, 100] {
        let lines = size_mb * 1024 * 1024 / 12; // ~12 bytes per line
        let data = generate_text(lines, 1);
        group.bench_with_input(
            BenchmarkId::new("memchr", format!("{}MB", size_mb)),
            &data,
            |b, data| b.iter(|| wc::count_lines(black_box(data))),
        );
    }
    group.finish();
}

fn bench_count_words(c: &mut Criterion) {
    let mut group = c.benchmark_group("wc_words");
    for size_mb in [1, 10] {
        let lines = size_mb * 1024 * 1024 / 60; // ~60 bytes per line with 5 words
        let data = generate_text(lines, 5);
        group.bench_with_input(
            BenchmarkId::new("scalar", format!("{}MB", size_mb)),
            &data,
            |b, data| b.iter(|| wc::count_words(black_box(data))),
        );
    }
    group.finish();
}

fn bench_count_bytes(c: &mut Criterion) {
    let data = generate_text(100_000, 5);
    c.bench_function("wc_bytes", |b| b.iter(|| wc::count_bytes(black_box(&data))));
}

fn bench_count_chars(c: &mut Criterion) {
    let mut group = c.benchmark_group("wc_chars");
    // ASCII data
    let ascii_data = generate_text(100_000, 5);
    group.bench_function("ascii_1MB", |b| {
        b.iter(|| wc::count_chars(black_box(&ascii_data), true))
    });

    // UTF-8 data with multibyte chars
    let utf8_text = "\u{4e16}\u{754c}\u{4f60}\u{597d} hello world\n".repeat(50_000);
    let utf8_data = utf8_text.as_bytes();
    group.bench_function("utf8_mixed", |b| {
        b.iter(|| wc::count_chars(black_box(utf8_data), true))
    });
    group.finish();
}

fn bench_count_all(c: &mut Criterion) {
    let data = generate_text(100_000, 5);
    c.bench_function("wc_count_all_1MB", |b| {
        b.iter(|| wc::count_all(black_box(&data), false))
    });
}

fn bench_max_line_length(c: &mut Criterion) {
    let data = generate_text(100_000, 10);
    c.bench_function("wc_max_line_length", |b| {
        b.iter(|| wc::max_line_length(black_box(&data), false))
    });
}

criterion_group!(
    benches,
    bench_count_lines,
    bench_count_words,
    bench_count_bytes,
    bench_count_chars,
    bench_count_all,
    bench_max_line_length,
);
criterion_main!(benches);
