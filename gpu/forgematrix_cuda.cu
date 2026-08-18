#include <cuda_runtime.h>

#include <cstdint>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

void cuda_check(cudaError_t result, const char* operation) {
    if (result != cudaSuccess) {
        throw std::runtime_error(std::string(operation) + ": " + cudaGetErrorString(result));
    }
}

uint32_t read_u32(const std::vector<uint8_t>& bytes, size_t& offset) {
    if (offset + 4 > bytes.size()) throw std::runtime_error("truncated u32");
    const uint32_t value = uint32_t(bytes[offset]) | (uint32_t(bytes[offset + 1]) << 8) |
                           (uint32_t(bytes[offset + 2]) << 16) |
                           (uint32_t(bytes[offset + 3]) << 24);
    offset += 4;
    return value;
}

std::vector<uint8_t> read_file(const char* path) {
    std::ifstream stream(path, std::ios::binary | std::ios::ate);
    if (!stream) throw std::runtime_error("could not open fixture");
    const auto length = stream.tellg();
    if (length < 0) throw std::runtime_error("could not size fixture");
    std::vector<uint8_t> bytes(static_cast<size_t>(length));
    stream.seekg(0);
    stream.read(reinterpret_cast<char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
    if (!stream) throw std::runtime_error("could not read fixture");
    return bytes;
}

__device__ int8_t quantize_nonzero(int64_t accumulator, uint8_t mask, uint32_t layer,
                                   uint32_t row, uint32_t col) {
    const int64_t coordinate_mix = int64_t(layer) * 0x1f123bb5LL +
                                   int64_t(row) * 0x05491333LL +
                                   int64_t(col) * 0x0127a2f1LL;
    int64_t value = (accumulator + int64_t(mask) + coordinate_mix) % 254;
    if (value < 0) value += 254;
    value -= 127;
    return value == 0 ? int8_t{1} : static_cast<int8_t>(value);
}

__global__ void forgematrix_layer(const int8_t* input, const int8_t* weights,
                                  const uint8_t* masks, int8_t* output, uint32_t rows,
                                  uint32_t width, uint32_t layer) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t values = rows * width;
    if (index >= values) return;
    const uint32_t row = index / width;
    const uint32_t col = index % width;
    int64_t accumulator = 0;
    for (uint32_t common = 0; common < width; ++common) {
        accumulator += int64_t(input[row * width + common]) *
                       int64_t(weights[common * width + col]);
    }
    output[index] = quantize_nonzero(accumulator, masks[index], layer, row, col);
}

}  // namespace

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            std::cerr << "usage: cmfd-forgematrix-cuda <gpu-fixture.bin>\n";
            return 2;
        }

        const auto fixture = read_file(argv[1]);
        if (fixture.size() < 20 ||
            std::string(reinterpret_cast<const char*>(fixture.data()), 8) != "CMFDGPU1") {
            throw std::runtime_error("bad fixture magic");
        }

        size_t offset = 8;
        const uint32_t width = read_u32(fixture, offset);
        const uint32_t rows = read_u32(fixture, offset);
        const uint32_t layers = read_u32(fixture, offset);
        const size_t activation_bytes = size_t(rows) * width;
        const size_t weight_bytes = size_t(width) * width;
        const size_t expected_size = 20 + activation_bytes + size_t(layers) *
            (weight_bytes + activation_bytes) + activation_bytes;
        if (fixture.size() != expected_size) throw std::runtime_error("fixture length mismatch");

        int8_t* d_input = nullptr;
        int8_t* d_output = nullptr;
        int8_t* d_weights = nullptr;
        uint8_t* d_masks = nullptr;
        cuda_check(cudaMalloc(&d_input, activation_bytes), "cudaMalloc input");
        cuda_check(cudaMalloc(&d_output, activation_bytes), "cudaMalloc output");
        cuda_check(cudaMalloc(&d_weights, weight_bytes), "cudaMalloc weights");
        cuda_check(cudaMalloc(&d_masks, activation_bytes), "cudaMalloc masks");
        cuda_check(cudaMemcpy(d_input, fixture.data() + offset, activation_bytes,
                              cudaMemcpyHostToDevice), "copy input");
        offset += activation_bytes;

        for (uint32_t layer = 0; layer < layers; ++layer) {
            cuda_check(cudaMemcpy(d_weights, fixture.data() + offset, weight_bytes,
                                  cudaMemcpyHostToDevice), "copy weights");
            offset += weight_bytes;
            cuda_check(cudaMemcpy(d_masks, fixture.data() + offset, activation_bytes,
                                  cudaMemcpyHostToDevice), "copy masks");
            offset += activation_bytes;

            constexpr uint32_t threads = 256;
            const uint32_t blocks = uint32_t((activation_bytes + threads - 1) / threads);
            forgematrix_layer<<<blocks, threads>>>(d_input, d_weights, d_masks, d_output,
                                                   rows, width, layer);
            cuda_check(cudaGetLastError(), "launch forgematrix_layer");
            std::swap(d_input, d_output);
        }

        std::vector<uint8_t> actual(activation_bytes);
        cuda_check(cudaMemcpy(actual.data(), d_input, activation_bytes, cudaMemcpyDeviceToHost),
                   "copy output");
        cuda_check(cudaDeviceSynchronize(), "synchronize");

        const uint8_t* expected = fixture.data() + offset;
        size_t differences = 0;
        for (size_t i = 0; i < activation_bytes; ++i) {
            if (actual[i] != expected[i]) ++differences;
        }

        cudaFree(d_masks);
        cudaFree(d_weights);
        cudaFree(d_output);
        cudaFree(d_input);

        if (differences != 0) {
            std::cerr << "FAIL cpu_gpu_differences=" << differences << "\n";
            return 1;
        }
        std::cout << "PASS dimensions=" << rows << "x" << width
                  << " layers=" << layers << " compared=" << activation_bytes << "\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "ERROR " << error.what() << "\n";
        return 1;
    }
}
