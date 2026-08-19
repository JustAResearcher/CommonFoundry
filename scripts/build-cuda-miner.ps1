[CmdletBinding()]
param(
    [string]$BuildDirectory,
    [ValidateSet('Release', 'Debug')]
    [string]$Configuration = 'Release',
    [switch]$SkipDifferentialTest
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
if (-not $BuildDirectory) {
    $BuildDirectory = Join-Path $projectRoot 'target\gpu-miner-build'
}
$BuildDirectory = [System.IO.Path]::GetFullPath($BuildDirectory)

$nvccCommand = Get-Command nvcc.exe -ErrorAction SilentlyContinue
if (-not $nvccCommand) {
    $fallback = 'C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2\bin\nvcc.exe'
    if (Test-Path -LiteralPath $fallback) {
        $nvcc = $fallback
    } else {
        throw 'nvcc.exe was not found. Install a CUDA toolkit that supports sm_120.'
    }
} else {
    $nvcc = $nvccCommand.Source
}

$supported = @(& $nvcc --list-gpu-code)
foreach ($architecture in @('sm_75', 'sm_86', 'sm_89', 'sm_120')) {
    if ($supported -notcontains $architecture) {
        throw "The selected CUDA toolkit cannot emit $architecture. CUDA 13.2 is recommended."
    }
}

$devCommandCandidates = @(
    'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat',
    'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat'
)
$devCommand = $devCommandCandidates |
    Where-Object { Test-Path -LiteralPath $_ } |
    Select-Object -First 1
if (-not $devCommand) {
    throw 'Visual Studio C++ developer environment was not found.'
}

$gpuSource = Join-Path $projectRoot 'gpu'
$configureAndBuild = '"{0}" -arch=x64 -host_arch=x64 && cmake -S "{1}" -B "{2}" -G Ninja -DCMAKE_BUILD_TYPE={3} -DCMAKE_CUDA_COMPILER="{4}" && cmake --build "{2}" --target cmfd-forgematrix-v2-miner' -f `
    $devCommand, $gpuSource, $BuildDirectory, $Configuration, $nvcc
& cmd.exe /d /s /c $configureAndBuild
if ($LASTEXITCODE -ne 0) {
    throw "CUDA miner build failed with exit code $LASTEXITCODE."
}

$library = Join-Path $BuildDirectory 'cmfd-forgematrix-v2-miner.dll'
if (-not (Test-Path -LiteralPath $library)) {
    throw "CUDA miner library was not produced at $library"
}

$cudaBin = Split-Path -Parent $nvcc
$cuobjdump = Join-Path $cudaBin 'cuobjdump.exe'
if (-not (Test-Path -LiteralPath $cuobjdump)) {
    throw 'cuobjdump.exe was not found beside nvcc.exe.'
}
$nativeImages = @(& $cuobjdump --list-elf $library)
$ptxImages = @(& $cuobjdump --list-ptx $library)
foreach ($architecture in @('sm_75', 'sm_86', 'sm_89', 'sm_120')) {
    if (-not ($nativeImages -match [regex]::Escape($architecture))) {
        throw "The library is missing its native $architecture image."
    }
}
if (-not ($ptxImages -match 'sm_75')) {
    throw 'The library is missing its forward-compatible compute_75 PTX image.'
}

if (-not $SkipDifferentialTest) {
    Push-Location $projectRoot
    try {
        $previousLibrary = $env:CMFD_CUDA_MINER_LIBRARY
        $env:CMFD_CUDA_MINER_LIBRARY = $library
        & cargo test -p common-foundry-wallet `
            cuda::tests::available_cuda_backend_matches_authoritative_v2_digests -- --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw "CUDA differential test failed with exit code $LASTEXITCODE."
        }
    } finally {
        $env:CMFD_CUDA_MINER_LIBRARY = $previousLibrary
        Pop-Location
    }
}

$file = Get-Item -LiteralPath $library
$stream = [System.IO.File]::OpenRead($library)
try {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $digest = $sha256.ComputeHash($stream)
    } finally {
        $sha256.Dispose()
    }
} finally {
    $stream.Dispose()
}
$hash = -join ($digest | ForEach-Object { $_.ToString('x2') })
[pscustomobject]@{
    Library = $file.FullName
    Bytes = $file.Length
    SHA256 = $hash
    NativeArchitectures = 'sm_75, sm_86, sm_89, sm_120'
    PtxFallback = 'compute_75'
}
