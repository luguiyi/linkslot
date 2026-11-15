# Windows 入口（P2-07）：探测环境后转发给 build/dist.sh。
#
#   powershell -ExecutionPolicy Bypass -File build\dist.ps1            # 出 h700 整包
#   powershell -ExecutionPolicy Bypass -File build\dist.ps1 host       # 桌面构建
#   powershell -ExecutionPolicy Bypass -File build\dist.ps1 --dry-run  # 只打印命令
#
# 优先使用 PATH 里的 sh（Git Bash / MSYS），否则尝试 WSL。

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Args
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$sh = Get-Command sh -ErrorAction SilentlyContinue
if ($sh) {
    Push-Location $root
    try {
        & $sh.Source 'build/dist.sh' @Args
        exit $LASTEXITCODE
    }
    finally {
        Pop-Location
    }
}

$wsl = Get-Command wsl -ErrorAction SilentlyContinue
if ($wsl) {
    $linux = (& wsl -d Ubuntu -- wslpath -a $root 2>$null)
    if ($LASTEXITCODE -eq 0 -and $linux) {
        $quoted = ($Args | ForEach-Object { "'" + ($_ -replace "'", "'\''") + "'" }) -join ' '
        & wsl -d Ubuntu -- bash -lc "cd '$linux' && sh build/dist.sh $quoted"
        exit $LASTEXITCODE
    }
}

Write-Error "找不到 sh 或 WSL。请安装 Docker Desktop / Git for Windows，或直接在 WSL 里运行 build/dist.sh。"
exit 1
