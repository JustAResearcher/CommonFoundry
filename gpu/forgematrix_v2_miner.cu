#include <cuda_runtime.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <new>
#include <stdexcept>
#include <string>
#include <vector>

#if defined(_WIN32)
#define CMFD_CUDA_EXPORT extern "C" __declspec(dllexport)
#else
#define CMFD_CUDA_EXPORT extern "C" __attribute__((visibility("default")))
#endif

namespace {

constexpr uint32_t API_VERSION = 1;
constexpr uint32_t MAX_WIDTH = 32;
constexpr uint32_t MAX_ROWS = 4;
constexpr uint32_t MAX_LAYERS = 4;
constexpr uint32_t MAX_ACTIVATION_VALUES = MAX_WIDTH * MAX_ROWS;
constexpr uint32_t MAX_BATCH = 65'536;
constexpr uint32_t THREADS = 128;

struct Context {
    int device_index = 0;
    uint32_t rows = 0;
    uint32_t width = 0;
    uint32_t layers = 0;
    uint32_t coefficient_count = 0;
    uint32_t activation_len = 0;
    uint32_t capacity = 0;
    uint8_t* device_base = nullptr;
    int8_t* device_weights = nullptr;
    uint8_t* device_coefficients = nullptr;
    uint8_t* device_outputs = nullptr;

    ~Context() {
        if (device_index >= 0) cudaSetDevice(device_index);
        cudaFree(device_outputs);
        cudaFree(device_coefficients);
        cudaFree(device_weights);
        cudaFree(device_base);
    }
};

void write_error(char* output, size_t output_len, const std::string& message) {
    if (output == nullptr || output_len == 0) return;
    const size_t copied = std::min(output_len - 1, message.size());
    std::memcpy(output, message.data(), copied);
    output[copied] = '\0';
}

void cuda_check(cudaError_t result, const char* operation) {
    if (result != cudaSuccess) {
        throw std::runtime_error(std::string(operation) + ": " + cudaGetErrorString(result));
    }
}

uint32_t exact_log2(uint32_t value) {
    if (value == 0 || (value & (value - 1)) != 0) {
        throw std::runtime_error("dimensions must be powers of two");
    }
    uint32_t bits = 0;
    while ((uint32_t{1} << bits) != value) ++bits;
    return bits;
}

void validate_canonical(const uint8_t* values, size_t length, const char* label) {
    if (values == nullptr) throw std::runtime_error(std::string(label) + " is null");
    for (size_t index = 0; index < length; ++index) {
        if (values[index] > 250) {
            throw std::runtime_error(std::string(label) + " contains a noncanonical byte");
        }
    }
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

__device__ int32_t pack_int8x4(const int8_t* values) {
    const uint32_t packed = uint32_t(uint8_t(values[0])) |
                            (uint32_t(uint8_t(values[1])) << 8) |
                            (uint32_t(uint8_t(values[2])) << 16) |
                            (uint32_t(uint8_t(values[3])) << 24);
    return static_cast<int32_t>(packed);
}

__global__ void evaluate_batch(const uint8_t* base, const int8_t* transposed_weights,
                               const uint8_t* all_coefficients, uint8_t* outputs, uint32_t rows,
                               uint32_t width, uint32_t layers, uint32_t coefficient_count,
                               uint32_t batch_count) {
    const uint32_t nonce_index = blockIdx.x;
    if (nonce_index >= batch_count) return;

    __shared__ int8_t activation[2][MAX_ACTIVATION_VALUES];
    const uint32_t index = threadIdx.x;
    const uint32_t activation_len = rows * width;
    const uint32_t row_bits = __ffs(static_cast<int>(rows)) - 1;
    const uint32_t col_bits = __ffs(static_cast<int>(width)) - 1;
    const uint32_t stages = layers + 1;
    const uint8_t* nonce_coefficients =
        all_coefficients + size_t(nonce_index) * stages * coefficient_count;

    if (index < activation_len) {
        const uint32_t row = index / width;
        const uint32_t col = index % width;
        const int32_t z = int32_t(base[index]) - 125 +
                          coordinate_mask(nonce_coefficients, row, col, row_bits, col_bits);
        activation[0][index] = cubic_reduce(z);
    }
    __syncthreads();

    for (uint32_t layer = 0; layer < layers; ++layer) {
        const uint32_t input_bank = layer & 1U;
        const uint32_t output_bank = input_bank ^ 1U;
        if (index < activation_len) {
            const uint32_t row = index / width;
            const uint32_t col = index % width;
            const int8_t* input = &activation[input_bank][row * width];
            const int8_t* weights = transposed_weights +
                                    size_t(layer) * width * width + size_t(col) * width;
            int32_t accumulator = 0;
            uint32_t common = 0;
            for (; common + 4 <= width; common += 4) {
                accumulator = __dp4a(pack_int8x4(input + common),
                                     pack_int8x4(weights + common), accumulator);
            }
            for (; common < width; ++common) {
                accumulator += int32_t(input[common]) * int32_t(weights[common]);
            }
            const uint8_t* coefficients =
                nonce_coefficients + size_t(layer + 1) * coefficient_count;
            const int32_t z = accumulator +
                              coordinate_mask(coefficients, row, col, row_bits, col_bits);
            activation[output_bank][index] = cubic_reduce(z);
        }
        __syncthreads();
    }

    if (index < activation_len) {
        outputs[size_t(nonce_index) * activation_len + index] =
            static_cast<uint8_t>(int32_t(activation[layers & 1U][index]) + 125);
    }
}

void ensure_capacity(Context& context, uint32_t count) {
    if (context.capacity >= count) return;
    cudaFree(context.device_outputs);
    cudaFree(context.device_coefficients);
    context.device_outputs = nullptr;
    context.device_coefficients = nullptr;
    context.capacity = 0;

    const size_t coefficient_bytes = size_t(count) * (context.layers + 1) *
                                     context.coefficient_count;
    const size_t output_bytes = size_t(count) * context.activation_len;
    cuda_check(cudaMalloc(&context.device_coefficients, coefficient_bytes),
               "allocate batch coefficients");
    try {
        cuda_check(cudaMalloc(&context.device_outputs, output_bytes), "allocate batch outputs");
    } catch (...) {
        cudaFree(context.device_coefficients);
        context.device_coefficients = nullptr;
        throw;
    }
    context.capacity = count;
}

}  // namespace

struct CmfdCudaDeviceInfo {
    uint32_t api_version;
    int32_t device_index;
    uint32_t compute_major;
    uint32_t compute_minor;
    uint64_t total_memory_bytes;
    char name[128];
};

CMFD_CUDA_EXPORT uint32_t cmfd_cuda_api_version() { return API_VERSION; }

CMFD_CUDA_EXPORT int32_t cmfd_cuda_device_count(int32_t* count, char* error, size_t error_len) {
    try {
        if (count == nullptr) throw std::runtime_error("device count output is null");
        int value = 0;
        cuda_check(cudaGetDeviceCount(&value), "enumerate CUDA devices");
        *count = value;
        return 0;
    } catch (const std::exception& exception) {
        write_error(error, error_len, exception.what());
        return 1;
    }
}

CMFD_CUDA_EXPORT int32_t cmfd_cuda_device_info(int32_t device_index,
                                                CmfdCudaDeviceInfo* output, char* error,
                                                size_t error_len) {
    try {
        if (output == nullptr) throw std::runtime_error("device info output is null");
        cudaDeviceProp properties{};
        cuda_check(cudaGetDeviceProperties(&properties, device_index), "read CUDA device");
        std::memset(output, 0, sizeof(*output));
        output->api_version = API_VERSION;
        output->device_index = device_index;
        output->compute_major = static_cast<uint32_t>(properties.major);
        output->compute_minor = static_cast<uint32_t>(properties.minor);
        output->total_memory_bytes = static_cast<uint64_t>(properties.totalGlobalMem);
        std::strncpy(output->name, properties.name, sizeof(output->name) - 1);
        return 0;
    } catch (const std::exception& exception) {
        write_error(error, error_len, exception.what());
        return 1;
    }
}

CMFD_CUDA_EXPORT int32_t cmfd_cuda_create(
    int32_t device_index, uint32_t rows, uint32_t width, uint32_t layers,
    uint32_t coefficient_count, const uint8_t* base_input, size_t base_input_len,
    const uint8_t* weights, size_t weights_len, void** output_context, char* error,
    size_t error_len) {
    try {
        if (output_context == nullptr) throw std::runtime_error("context output is null");
        *output_context = nullptr;
        if (rows == 0 || rows > MAX_ROWS || width == 0 || width > MAX_WIDTH || layers == 0 ||
            layers > MAX_LAYERS) {
            throw std::runtime_error("CUDA backend accepts only the bounded v2 research profile");
        }
        const uint32_t row_bits = exact_log2(rows);
        const uint32_t col_bits = exact_log2(width);
        if (coefficient_count != 1 + row_bits + col_bits) {
            throw std::runtime_error("coefficient count mismatch");
        }
        const size_t activation_len = size_t(rows) * width;
        const size_t expected_weights = size_t(layers) * width * width;
        if (base_input_len != activation_len || weights_len != expected_weights) {
            throw std::runtime_error("model byte length mismatch");
        }
        validate_canonical(base_input, base_input_len, "base input");
        validate_canonical(weights, weights_len, "weights");

        cuda_check(cudaSetDevice(device_index), "select CUDA device");
        cudaDeviceProp properties{};
        cuda_check(cudaGetDeviceProperties(&properties, device_index), "read CUDA device");
        if (properties.major < 7) {
            throw std::runtime_error("CUDA device must support compute capability 7.0 or newer");
        }

        auto context = std::make_unique<Context>();
        context->device_index = device_index;
        context->rows = rows;
        context->width = width;
        context->layers = layers;
        context->coefficient_count = coefficient_count;
        context->activation_len = static_cast<uint32_t>(activation_len);

        std::vector<int8_t> transposed_weights(expected_weights);
        for (uint32_t layer = 0; layer < layers; ++layer) {
            for (uint32_t col = 0; col < width; ++col) {
                for (uint32_t common = 0; common < width; ++common) {
                    const size_t source = size_t(layer) * width * width + size_t(common) * width + col;
                    const size_t target = size_t(layer) * width * width + size_t(col) * width + common;
                    transposed_weights[target] = static_cast<int8_t>(int32_t(weights[source]) - 125);
                }
            }
        }

        cuda_check(cudaMalloc(&context->device_base, activation_len), "allocate base input");
        cuda_check(cudaMalloc(&context->device_weights, expected_weights), "allocate weights");
        cuda_check(cudaMemcpy(context->device_base, base_input, activation_len,
                              cudaMemcpyHostToDevice),
                   "copy base input");
        cuda_check(cudaMemcpy(context->device_weights, transposed_weights.data(), expected_weights,
                              cudaMemcpyHostToDevice),
                   "copy weights");
        *output_context = context.release();
        return 0;
    } catch (const std::exception& exception) {
        write_error(error, error_len, exception.what());
        return 1;
    }
}

CMFD_CUDA_EXPORT int32_t cmfd_cuda_evaluate(void* opaque_context,
                                             const uint8_t* coefficients,
                                             size_t coefficients_len, uint32_t count,
                                             uint8_t* outputs, size_t outputs_len, char* error,
                                             size_t error_len) {
    try {
        if (opaque_context == nullptr) throw std::runtime_error("CUDA context is null");
        if (count == 0 || count > MAX_BATCH) throw std::runtime_error("batch size is out of range");
        auto& context = *static_cast<Context*>(opaque_context);
        const size_t expected_coefficients =
            size_t(count) * (context.layers + 1) * context.coefficient_count;
        const size_t expected_outputs = size_t(count) * context.activation_len;
        if (coefficients == nullptr || coefficients_len != expected_coefficients ||
            outputs == nullptr || outputs_len != expected_outputs) {
            throw std::runtime_error("batch buffer length mismatch");
        }
        validate_canonical(coefficients, coefficients_len, "mask coefficients");
        cuda_check(cudaSetDevice(context.device_index), "select CUDA device");
        ensure_capacity(context, count);
        cuda_check(cudaMemcpy(context.device_coefficients, coefficients, coefficients_len,
                              cudaMemcpyHostToDevice),
                   "copy mask coefficients");
        evaluate_batch<<<count, THREADS>>>(
            context.device_base, context.device_weights, context.device_coefficients,
            context.device_outputs, context.rows, context.width, context.layers,
            context.coefficient_count, count);
        cuda_check(cudaGetLastError(), "launch ForgeMatrix v2 batch");
        cuda_check(cudaMemcpy(outputs, context.device_outputs, outputs_len, cudaMemcpyDeviceToHost),
                   "copy ForgeMatrix v2 outputs");
        return 0;
    } catch (const std::exception& exception) {
        write_error(error, error_len, exception.what());
        return 1;
    }
}

CMFD_CUDA_EXPORT void cmfd_cuda_destroy(void* opaque_context) {
    delete static_cast<Context*>(opaque_context);
}
