cmake_minimum_required(VERSION 3.22)
project(cmfd_forgematrix_cuda LANGUAGES CXX CUDA)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CUDA_STANDARD 17)
set(CMAKE_CUDA_STANDARD_REQUIRED ON)

add_executable(cmfd-forgematrix-cuda forgematrix_cuda.cu)
add_executable(cmfd-forgematrix-v2-cuda forgematrix_v2_cuda.cu)
target_compile_options(cmfd-forgematrix-cuda PRIVATE
  $<$<COMPILE_LANGUAGE:CUDA>:--fmad=false>
)
target_compile_options(cmfd-forgematrix-v2-cuda PRIVATE
  $<$<COMPILE_LANGUAGE:CUDA>:--fmad=false>
)
set_target_properties(cmfd-forgematrix-cuda PROPERTIES
  CUDA_ARCHITECTURES "89;120"
)
set_target_properties(cmfd-forgematrix-v2-cuda PROPERTIES
  CUDA_ARCHITECTURES "89;120"
)
