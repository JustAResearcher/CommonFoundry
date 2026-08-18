[CmdletBinding()]
param(
    [ValidateSet('v1', 'v2')]
    [string]$Version = 'v1'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$targetDir = Join-Path $projectRoot 'target'
if ($Version -eq 'v2') {
    $fixturePath = Join-Path $targetDir 'gpu-fixture-v2.bin'
    $binaryPath = Join-Path $targetDir 'cmfd-forgematrix-v2-cuda.exe'
    $cudaSource = Join-Path $projectRoot 'gpu\forgematrix_v2_cuda.cu'
    $fixtureCommand = 'gpu-fixture-v2'
} else {
    $fixturePath = Join-Path $targetDir 'gpu-fixture.bin'
    $binaryPath = Join-Path $targetDir 'cmfd-forgematrix-cuda.exe'
    $cudaSource = Join-Path $projectRoot 'gpu\forgematrix_cuda.cu'
    $fixtureCommand = 'gpu-fixture'
}

Push-Location $projectRoot
try {
    & cargo run --quiet -p cmfd-consensus -- $fixtureCommand --output $fixturePath
    if ($LASTEXITCODE -ne 0) { throw "fixture generation failed with exit code $LASTEXITCODE" }

    $nvcc = (Get-Command nvcc.exe -ErrorAction SilentlyContinue).Source
    if (-not $nvcc) {
        $nvcc = 'C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2\bin\nvcc.exe'
    }
    if (-not (Test-Path -LiteralPath $nvcc)) { throw "nvcc.exe was not found" }

    $devCmdCandidates = @(
        'C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat',
        'C:\Program Files\Microsoft Visual Studio\18\Community\Common7\Tools\VsDevCmd.bat'
    )
    $devCmd = $devCmdCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
    if (-not $devCmd) { throw "Visual Studio C++ developer environment was not found" }

    $architecture = (nvidia-smi --query-gpu=compute_cap --format=csv,noheader | Select-Object -First 1).Trim().Replace('.', '')
    if (-not $architecture) { throw "could not determine CUDA compute capability" }

    $compile = '"{0}" -arch=x64 -host_arch=x64 && "{1}" -O3 -std=c++17 -arch=sm_{2} --fmad=false "{3}" -o "{4}"' -f `
        $devCmd, $nvcc, $architecture, $cudaSource, $binaryPath
    & cmd.exe /d /s /c $compile
    if ($LASTEXITCODE -ne 0) { throw "CUDA compilation failed with exit code $LASTEXITCODE" }

    & $binaryPath $fixturePath
    if ($LASTEXITCODE -ne 0) { throw "CPU/GPU differential test failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}
