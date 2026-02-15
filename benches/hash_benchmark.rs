use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::path::Path;

use coreutils_rs::hash::{self, HashAlgorithm};

/// Create test data of the given size for benchmarking.
fn make_test_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

fn bench_hash_bytes(c: &mut Criterion) {
    let sizes = [1024, 64 * 1024, 1024 * 1024, 10 * 1024 * 1024];

    let mut group = c.benchmark_group("hash_bytes");
    for &size in &sizes {
        let data = make_test_data(size);
        let label = if size >= 1024 * 1024 {
            format!("{}MB", size / (1024 * 1024))
        } else {
            format!("{}KB", size / 1024)
        };

        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("sha256", &label), &data, |b, data| {
            b.iter(|| hash::hash_bytes(HashAlgorithm::Sha256, data));
        });

        group.bench_with_input(BenchmarkId::new("md5", &label), &data, |b, data| {
            b.iter(|| hash::hash_bytes(HashAlgorithm::Md5, data));
        });

        group.bench_with_input(BenchmarkId::new("blake2b", &label), &data, |b, data| {
            b.iter(|| hash::hash_bytes(HashAlgorithm::Blake2b, data));
        });
    }
    group.finish();
}

fn bench_hash_file(c: &mut Criterion) {
    // Use pre-existing benchmark files if available
    let test_files: Vec<(&str, &str)> = vec![
        ("10MB", "/tmp/bench-data/file_1.bin"),
        ("100MB", "/tmp/bench-data/large_1.bin"),
    ];

    let mut group = c.benchmark_group("hash_file");
    group.sample_size(10); // Fewer samples for large files

    for (label, path) in &test_files {
        let file_path = Path::new(path);
        if !file_path.exists() {
            continue;
        }
        let size = std::fs::metadata(file_path).unwrap().len();
        group.throughput(Throughput::Bytes(size));

        group.bench_with_input(BenchmarkId::new("sha256", label), file_path, |b, path| {
            b.iter(|| hash::hash_file(HashAlgorithm::Sha256, path).unwrap());
        });

        group.bench_with_input(BenchmarkId::new("md5", label), file_path, |b, path| {
            b.iter(|| hash::hash_file(HashAlgorithm::Md5, path).unwrap());
        });

        group.bench_with_input(BenchmarkId::new("blake2b", label), file_path, |b, path| {
            b.iter(|| hash::hash_file(HashAlgorithm::Blake2b, path).unwrap());
        });
    }
    group.finish();
}

/// Benchmark single-file hash paths: hash_file vs hash_file_nostat vs hash_file_raw.
/// Creates temporary files at each size tier to measure real I/O + hash performance.
fn bench_single_file_hash(c: &mut Criterion) {
    let sizes: Vec<(usize, &str)> = vec![
        (55, "55B"),
        (4096, "4KB"),
        (65536, "64KB"),
        (1024 * 1024, "1MB"),
    ];

    // Create temp files
    let dir = tempfile::tempdir().unwrap();
    let test_files: Vec<(std::path::PathBuf, &str)> = sizes
        .iter()
        .map(|(size, label)| {
            let path = dir.path().join(format!("bench-{}", label));
            let data = make_test_data(*size);
            std::fs::write(&path, &data).unwrap();
            (path, *label)
        })
        .collect();

    let mut group = c.benchmark_group("single_file_md5");
    for (path, label) in &test_files {
        let size = std::fs::metadata(path).unwrap().len();
        group.throughput(Throughput::Bytes(size));

        // hash_file (open_and_stat path)
        group.bench_with_input(BenchmarkId::new("hash_file", label), path, |b, path| {
            b.iter(|| hash::hash_file(HashAlgorithm::Md5, path).unwrap());
        });

        // hash_file_nostat (skip fstat path)
        group.bench_with_input(
            BenchmarkId::new("hash_file_nostat", label),
            path,
            |b, path| {
                b.iter(|| hash::hash_file_nostat(HashAlgorithm::Md5, path).unwrap());
            },
        );

        // hash_file_raw (raw syscall path — Linux only)
        #[cfg(target_os = "linux")]
        group.bench_with_input(BenchmarkId::new("hash_file_raw", label), path, |b, path| {
            b.iter(|| hash::hash_file_raw(HashAlgorithm::Md5, path).unwrap());
        });

        // hash_file_raw_to_buf (zero-alloc path — Linux only)
        #[cfg(target_os = "linux")]
        group.bench_with_input(
            BenchmarkId::new("hash_file_raw_to_buf", label),
            path,
            |b, path| {
                let mut hex_buf = [0u8; 128];
                b.iter(|| {
                    hash::hash_file_raw_to_buf(HashAlgorithm::Md5, path, &mut hex_buf).unwrap()
                });
            },
        );
    }
    group.finish();
}

fn bench_parallel_hash(c: &mut Criterion) {
    use rayon::prelude::*;

    // Use 4 x 100MB files for parallel benchmark
    let files: Vec<String> = (1..=4)
        .map(|i| format!("/tmp/bench-data/large_{}.bin", i))
        .collect();

    // Skip if files don't exist
    if !files.iter().all(|f| Path::new(f).exists()) {
        return;
    }

    let total_size: u64 = files
        .iter()
        .map(|f| std::fs::metadata(f).unwrap().len())
        .sum();

    let mut group = c.benchmark_group("parallel_hash");
    group.sample_size(10);
    group.throughput(Throughput::Bytes(total_size));

    group.bench_function("sha256_4x100MB", |b| {
        b.iter(|| {
            let _results: Vec<_> = files
                .par_iter()
                .map(|f| hash::hash_file(HashAlgorithm::Sha256, Path::new(f)).unwrap())
                .collect();
        });
    });

    group.bench_function("md5_4x100MB", |b| {
        b.iter(|| {
            let _results: Vec<_> = files
                .par_iter()
                .map(|f| hash::hash_file(HashAlgorithm::Md5, Path::new(f)).unwrap())
                .collect();
        });
    });

    group.bench_function("blake2b_4x100MB", |b| {
        b.iter(|| {
            let _results: Vec<_> = files
                .par_iter()
                .map(|f| hash::hash_file(HashAlgorithm::Blake2b, Path::new(f)).unwrap())
                .collect();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_hash_bytes,
    bench_hash_file,
    bench_single_file_hash,
    bench_parallel_hash
);
criterion_main!(benches);
