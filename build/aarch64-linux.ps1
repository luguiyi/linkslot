# 一键构建 aarch64 Linux 产物的 Windows 入口。
#
# 默认优先使用 Docker Desktop（linux/arm64 容器）；没有 Docker 时回退 WSL。
# 产物仍写入 target/aarch64-linux/。
#
# 用法（仓库根目录）：
#   powershell -File build\aarch64-linux.ps1
#   powershell -File build\aarch64-linux.ps1 --no-cores --tar
#   powershell -File build\aarch64-linux.ps1 -Wsl        # 强制走 WSL
#   （PowerShell 7 也可用 pwsh -File ...）
#
# 注意：本脚本只是入口包装；真正的构建逻辑在 build/aarch64-linux.sh。

[CmdletBinding()]
param(
    [switch]$Wsl,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$BuildArgs
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

function Fail([string]$Message) {
    [Console]::Error.WriteLine("错误：$Message")
    exit 1
}

function Invoke-DockerBuild {
    $platform = if ($env:DOCKER_PLATFORM) { $env:DOCKER_PLATFORM } else { 'linux/arm64' }
    $image = if ($env:DOCKER_IMAGE) { $env:DOCKER_IMAGE } else { 'rust:1-bullseye' }
    Write-Host "通过 Docker 执行：$image ($platform)"
    $dockerArgs = @(
        'run', '--rm', '--platform', $platform,
        '-v', "${root}:/src", '-w', '/src',
        '-v', 'slot-aarch64-cargo-registry:/usr/local/cargo/registry',
        '-v', 'slot-aarch64-cargo-git:/usr/local/cargo/git',
        '-v', 'slot-aarch64-rustup:/usr/local/rustup',
        '-e', 'CARGO_HOME=/usr/local/cargo',
        $image, 'bash', 'build/aarch64-linux.sh', '--in-container'
    )
    if ($BuildArgs) { $dockerArgs += $BuildArgs }
    & docker @dockerArgs
    exit $LASTEXITCODE
}

# 1) 默认优先 Docker Desktop：Windows 上直接调用 docker.exe，路径由
#    Docker Desktop 负责映射，不需要额外的 WSL 发行版。
$docker = Get-Command docker -ErrorAction SilentlyContinue
if ($docker -and -not $Wsl) {
    Invoke-DockerBuild
}

# 2) 没有 Docker，或显式 -Wsl：通过 WSL 执行 shell 脚本（路径转成 /mnt/...）。
$wslCmd = Get-Command wsl.exe -ErrorAction SilentlyContinue
if ($wslCmd) {
    $wslRoot = $null
    try {
        $wslRoot = (& wsl.exe wslpath -u $root 2>$null | Out-String).Trim()
    } catch {
        $wslRoot = $null
    }
    if ($LASTEXITCODE -eq 0 -and $wslRoot) {
        Write-Host "通过 WSL 执行：bash $wslRoot/build/aarch64-linux.sh"
        $shellArgs = @('-e', 'bash', "$wslRoot/build/aarch64-linux.sh")
        if ($BuildArgs) { $shellArgs += $BuildArgs }
        & wsl.exe @shellArgs
        exit $LASTEXITCODE
    }
}

# 3) 显式 -Wsl 但 WSL 不可用：有 Docker Desktop 时仍然回退 Docker。
if ($docker) {
    Invoke-DockerBuild
}

Fail "需要 Docker Desktop 或可用的 WSL（含 bash）。也可在 aarch64 Linux 上直接运行 bash build/aarch64-linux.sh。"
