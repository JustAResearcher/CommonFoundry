#include <cuda_runtime.h>

#include <cstdint>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

constexpr size_t MAX_RESEARCH_FIXTURE_BYTES = 1024 * 1024;

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

uint32_t exact_log2(uint32_t value) {
    if (value == 0 || (value & (value - 1)) != 0) {
        throw std::runtime_error("dimensions must be powers of two");
    }
    uint32_t bits = 0;
    while ((uint32_t{1} << bits) != value) ++bits;
    return bits;
}

void validate_model_bytes(const std::vector<uint8_t>& fixture, size_t offset, size_t length,
                          const char* section) {
    if (offset > fixture.size() || length > fixture.size() - offset) {
        throw std::runtime_error(std::string("truncated ") + section);
    }
    for (size_t index = 0; index < length; ++index) {
        if (fixture[offset + index] > 250) {
            throw std::runtime_error(std::string("out-of-range byte in ") + section);
        }
    }
}

std::vector<uint8_t> read_file(const char* path) {
    std::ifstream stream(path, std::ios::binary | std::ios::ate);
    if (!stream) throw std::runtime_error("could not open fixture");
    const auto length = stream.tellg();
    if (length < 0) throw std::runtime_error("could not size fixture");
    if (static_cast<uint64_t>(length) > MAX_RESEARCH_FIXTURE_BYTES) {
        throw std::runtime_error("fixture exceeds the bounded research size");
    }
    std::vector<uint8_t> bytes(static_cast<size_t>(length));
    stream.seekg(0);
    stream.read(reinterpret_cast<char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
    if (!stream) throw std::runtime_error("could not read fixture");
    return bytes;
}

size_t compare_activation(const int8_t* device, const uint8_t* expected, size_t length) {
    std::vector<int8_t> actual(length);
    cuda_check(cudaMemcpy(actual.data(), device, length, cudaMemcpyDeviceToHost), "copy output");
    size_t differences = 0;
    for (size_t index = 0; index < length; ++index) {
        const uint8_t encoded = static_cast<uint8_t>(int32_t(actual[index]) + 125);
        if (encoded != expected[index]) ++differences;
    }
    return differences;
}

__device__ int32_t coordinate_mask(const uint8_t* coefficients, uint32_t row, uint32_t col,
                                   uint32_t row_bits, uint32_t col_bits) {
    int32_t mask = coefficients[0];
    for (uint32_t bit = 0; bit < row_bits; ++bit) {
        if (((row >> bit) & 1U) != 0) mask += coefficients[1 + bit];
    }
    for (uint32_t bit = 0; bit < col_bits; ++bit) {
        if (((col >> bit) & 1U) != 0) mask += coefficients[1 + row_bits + bit];
    }
    return mask;
}

__device__ int8_t cubic_reduce(int32_t z) {
    constexpr uint32_t transition_modulus = 134217689;
    const uint32_t encoded =
        z >= 0 ? static_cast<uint32_t>(z)
               : static_cast<uint32_t>(int64_t(transition_modulus) + int64_t(z));
    const uint64_t square = uint64_t(encoded) * encoded;
    const uint32_t square_remainder = static_cast<uint32_t>(square % transition_modulus);
    const uint64_t cube_product = uint64_t(square_remainder) * encoded;
    const uint32_t cube_remainder = static_cast<uint32_t>(cube_product % transition_modulus);
    return static_cast<int8_t>(static_cast<int32_t>(cube_remainder % 251) - 125);
}

__global__ void initialize_activation(const uint8_t* base, const uint8_t* coefficients,
                                      int8_t* output, uint32_t rows, uint32_t width,
                                      uint32_t row_bits, uint32_t col_bits) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t values = rows * width;
    if (index >= values) return;
    const uint32_t row = index / width;
    const uint32_t col = index % width;
    const int32_t z = int32_t(base[index]) - 125 +
                      coordinate_mask(coefficients, row, col, row_bits, col_bits);
    output[index] = cubic_reduce(z);
}

__global__ void forgematrix_v2_layer(const int8_t* input, const uint8_t* weights,
                                     const uint8_t* coefficients, int8_t* output,
                                     uint32_t rows, uint32_t width, uint32_t row_bits,
                                     uint32_t col_bits) {
    const uint32_t index = blockIdx.x * blockDim.x + threadIdx.x;
    const uint32_t values = rows * width;
    if (index >= values) return;
    const uint32_t row = index / width;
    const uint32_t col = index % width;
    int32_t accumulator = 0;
    for (uint32_t common = 0; common < width; ++common) {
        accumulator += int32_t(input[row * width + common]) *
                       (int32_t(weights[common * width + col]) - 125);
    }
    const int32_t z = accumulator +
                      coordinate_mask(coefficients, row, col, row_bits, col_bits);
    output[index] = cubic_reduce(z);
}

}  // namespace

int main(int argc, char** argv) {
    try {
        if (argc != 2) {
            std::cerr << "usage: cmfd-forgematrix-v2-cuda <gpu-fixture-v2.bin>\n";
            return 2;
        }

        const auto fixture = read_file(argv[1]);
        if (fixture.size() < 56 ||
            std::string(reinterpret_cast<const char*>(fixture.data()), 8) != "CMFDGPU2") {
            throw std::runtime_error("bad fixture magic");
        }

        size_t offset = 8;
        const uint32_t width = read_u32(fixture, offset);
        const uint32_t rows = read_u32(fixture, offset);
        const uint32_t layers = read_u32(fixture, offset);
        const uint32_t coefficient_count = read_u32(fixture, offset);
        if (width == 0 || width > 32 || rows == 0 || rows > 4 || layers == 0 || layers > 4) {
            throw std::runtime_error("v2 CUDA oracle accepts only the bounded research profile");
        }
        const uint32_t row_bits = exact_log2(rows);
        const uint32_t col_bits = exact_log2(width);
        if (coefficient_count != 1 + row_bits + col_bits) {
            throw std::runtime_error("coefficient count mismatch");
        }
        // This arithmetic oracle carries the challenge for fixture identity,
        // but independently checks only the supplied coefficients and layer
        // relation; BLAKE3 coefficient derivation remains a separate test gap.
        offset += 32;

        const size_t activation_bytes = size_t(rows) * width;
        const size_t weight_bytes = size_t(width) * width;
        const size_t expected_size = 56 + activation_bytes + coefficient_count +
                                     activation_bytes + size_t(layers) *
                                     (weight_bytes + coefficient_count + activation_bytes);
        if (fixture.size() != expected_size) throw std::runtime_error("fixture length mismatch");

        size_t validation_offset = offset;
        validate_model_bytes(fixture, validation_offset, activation_bytes, "base table");
        validation_offset += activation_bytes;
        validate_model_bytes(fixture, validation_offset, coefficient_count,
                             "input coefficients");
        validation_offset += coefficient_count;
        validate_model_bytes(fixture, validation_offset, activation_bytes,
                             "initialized activation");
        validation_offset += activation_bytes;
        for (uint32_t layer = 0; layer < layers; ++layer) {
            validate_model_bytes(fixture, validation_offset, weight_bytes, "weight matrix");
            validation_offset += weight_bytes;
            validate_model_bytes(fixture, validation_offset, coefficient_count,
                                 "layer coefficients");
            validation_offset += coefficient_count;
            validate_model_bytes(fixture, validation_offset, activation_bytes,
                                 "layer output");
            validation_offset += activation_bytes;
        }

        int8_t* d_input = nullptr;
        int8_t* d_output = nullptr;
        uint8_t* d_base = nullptr;
        uint8_t* d_weights = nullptr;
        uint8_t* d_coefficients = nullptr;
        cuda_check(cudaMalloc(&d_input, activation_bytes), "cudaMalloc input");
        cuda_check(cudaMalloc(&d_output, activation_bytes), "cudaMalloc output");
        cuda_check(cudaMalloc(&d_base, activation_bytes), "cudaMalloc base");
        cuda_check(cudaMalloc(&d_weights, weight_bytes), "cudaMalloc weights");
        cuda_check(cudaMalloc(&d_coefficients, coefficient_count), "cudaMalloc coefficients");

        cuda_check(cudaMemcpy(d_base, fixture.data() + offset, activation_bytes,
                              cudaMemcpyHostToDevice), "copy base");
        offset += activation_bytes;
        cuda_check(cudaMemcpy(d_coefficients, fixture.data() + offset, coefficient_count,
                              cudaMemcpyHostToDevice), "copy input coefficients");
        offset += coefficient_count;

        constexpr uint32_t threads = 256;
        const uint32_t blocks = uint32_t((activation_bytes + threads - 1) / threads);
        initialize_activation<<<blocks, threads>>>(d_base, d_coefficients, d_input, rows, width,
                                                   row_bits, col_bits);
        cuda_check(cudaGetLastError(), "launch initialize_activation");
        size_t differences = compare_activation(d_input, fixture.data() + offset, activation_bytes);
        offset += activation_bytes;

        for (uint32_t layer = 0; layer < layers; ++layer) {
            cuda_check(cudaMemcpy(d_weights, fixture.data() + offset, weight_bytes,
                                  cudaMemcpyHostToDevice), "copy weights");
            offset += weight_bytes;
            cuda_check(cudaMemcpy(d_coefficients, fixture.data() + offset, coefficient_count,
                                  cudaMemcpyHostToDevice), "copy layer coefficients");
            offset += coefficient_count;
            forgematrix_v2_layer<<<blocks, threads>>>(d_input, d_weights, d_coefficients, d_output,
                                                      rows, width, row_bits, col_bits);
            cuda_check(cudaGetLastError(), "launch forgematrix_v2_layer");
            std::swap(d_input, d_output);
            differences += compare_activation(d_input, fixture.data() + offset, activation_bytes);
            offset += activation_bytes;
        }
        cuda_check(cudaDeviceSynchronize(), "synchronize");

        cudaFree(d_coefficients);
        cudaFree(d_weights);
        cudaFree(d_base);
        cudaFree(d_output);
        cudaFree(d_input);

        if (differences != 0) {
            std::cerr << "FAIL cpu_gpu_differences=" << differences << "\n";
            return 1;
        }
        std::cout << "PASS v2 dimensions=" << rows << "x" << width
                  << " layers=" << layers
                  << " compared=" << activation_bytes * (size_t(layers) + 1) << "\n";
        return 0;
    } catch (const std::exception& error) {
        std::cerr << "ERROR " << error.what() << "\n";
        return 1;
    }
}
