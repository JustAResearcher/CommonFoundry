#!/usr/bin/env python3
"""Exercise the Common Foundry CUDA miner ABI with a canonical v2 fixture."""

from __future__ import annotations

import argparse
import ctypes
import json
import struct
import time
from pathlib import Path


API_VERSION = 1
MAX_BATCH = 65_536


class DeviceInfo(ctypes.Structure):
    _fields_ = [
        ("api_version", ctypes.c_uint32),
        ("device_index", ctypes.c_int32),
        ("compute_major", ctypes.c_uint32),
        ("compute_minor", ctypes.c_uint32),
        ("total_memory_bytes", ctypes.c_uint64),
        ("name", ctypes.c_char * 128),
    ]


def parse_fixture(path: Path) -> tuple[int, int, int, int, bytes, bytes, bytes, bytes]:
    data = path.read_bytes()
    if len(data) < 56 or data[:8] != b"CMFDGPU2":
        raise ValueError("fixture has the wrong magic or is truncated")
    width, rows, layers, coefficient_count = struct.unpack_from("<IIII", data, 8)
    activation_len = rows * width
    weight_len = width * width
    offset = 56
    base = data[offset : offset + activation_len]
    offset += activation_len
    coefficients = bytearray(data[offset : offset + coefficient_count])
    offset += coefficient_count + activation_len
    weights = bytearray()
    final_expected = b""
    for _ in range(layers):
        weights.extend(data[offset : offset + weight_len])
        offset += weight_len
        coefficients.extend(data[offset : offset + coefficient_count])
        offset += coefficient_count
        final_expected = data[offset : offset + activation_len]
        offset += activation_len
    if offset != len(data):
        raise ValueError("fixture has trailing or missing bytes")
    return rows, width, layers, coefficient_count, base, bytes(weights), bytes(coefficients), final_expected


def configure(library: ctypes.CDLL) -> None:
    library.cmfd_cuda_api_version.restype = ctypes.c_uint32
    library.cmfd_cuda_device_count.argtypes = [
        ctypes.POINTER(ctypes.c_int32), ctypes.c_char_p, ctypes.c_size_t
    ]
    library.cmfd_cuda_device_info.argtypes = [
        ctypes.c_int32, ctypes.POINTER(DeviceInfo), ctypes.c_char_p, ctypes.c_size_t
    ]
    library.cmfd_cuda_create.argtypes = [
        ctypes.c_int32,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_uint32,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    library.cmfd_cuda_evaluate.argtypes = [
        ctypes.c_void_p,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_uint32,
        ctypes.c_void_p,
        ctypes.c_size_t,
        ctypes.c_char_p,
        ctypes.c_size_t,
    ]
    library.cmfd_cuda_destroy.argtypes = [ctypes.c_void_p]


def checked(result: int, error: ctypes.Array[ctypes.c_char]) -> None:
    if result != 0:
        message = bytes(error).split(b"\0", 1)[0].decode("utf-8", errors="replace")
        raise RuntimeError(message or f"CUDA backend failed with code {result}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--library", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--device", type=int, default=0)
    parser.add_argument("--batch", type=int, default=4_096)
    args = parser.parse_args()
    if not 1 <= args.batch <= MAX_BATCH:
        parser.error(f"--batch must be between 1 and {MAX_BATCH}")

    rows, width, layers, coefficient_count, base, weights, coefficients, expected = parse_fixture(args.fixture)
    library = ctypes.CDLL(str(args.library.resolve()))
    configure(library)
    if library.cmfd_cuda_api_version() != API_VERSION:
        raise RuntimeError("CUDA backend API version mismatch")

    error = ctypes.create_string_buffer(512)
    count = ctypes.c_int32()
    checked(library.cmfd_cuda_device_count(ctypes.byref(count), error, len(error)), error)
    info = DeviceInfo()
    error.value = b""
    checked(library.cmfd_cuda_device_info(args.device, ctypes.byref(info), error, len(error)), error)

    base_buffer = ctypes.create_string_buffer(base)
    weights_buffer = ctypes.create_string_buffer(weights)
    context = ctypes.c_void_p()
    error.value = b""
    checked(
        library.cmfd_cuda_create(
            args.device,
            rows,
            width,
            layers,
            coefficient_count,
            base_buffer,
            len(base),
            weights_buffer,
            len(weights),
            ctypes.byref(context),
            error,
            len(error),
        ),
        error,
    )
    try:
        batch_coefficients = coefficients * args.batch
        coefficient_buffer = ctypes.create_string_buffer(batch_coefficients)
        output_len = rows * width * args.batch
        output_buffer = ctypes.create_string_buffer(output_len)
        started = time.perf_counter()
        error.value = b""
        checked(
            library.cmfd_cuda_evaluate(
                context,
                coefficient_buffer,
                len(batch_coefficients),
                args.batch,
                output_buffer,
                output_len,
                error,
                len(error),
            ),
            error,
        )
        elapsed = time.perf_counter() - started
        output = output_buffer.raw
        if output[: len(expected)] != expected:
            raise RuntimeError("CUDA output differs from the canonical Rust fixture")
        if any(output[index : index + len(expected)] != expected for index in range(0, len(output), len(expected))):
            raise RuntimeError("one or more repeated CUDA batch outputs differ")
    finally:
        library.cmfd_cuda_destroy(context)

    print(json.dumps({
        "result": "PASS",
        "cuda_devices": count.value,
        "selected_device": args.device,
        "name": bytes(info.name).split(b"\0", 1)[0].decode("utf-8", errors="replace"),
        "compute_capability": f"{info.compute_major}.{info.compute_minor}",
        "total_memory_bytes": info.total_memory_bytes,
        "batch": args.batch,
        "elapsed_seconds": elapsed,
        "matrix_stage_evaluations_per_second": args.batch / max(elapsed, 1e-12),
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
