# Meld installer for Windows:
#   irm https://raw.githubusercontent.com/siddhjagani/meld/main/install.ps1 | iex
# Downloads the latest release binary and adds it to your PATH.
# No administrator rights required.
$ErrorActionPreference = "Stop"

$Repo = if ($env:MELD_REPO) { $env:MELD_REPO } else { "siddhjagani/meld" }
$InstallDir = if ($env:MELD_INSTALL_DIR) { $env:MELD_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "meld\bin" }
$Target = "x86_64-pc-windows-msvc"

# Direct "latest" download link: no GitHub API call, so no API rate limit.
$AssetUrl = "https://github.com/$Repo/releases/latest/download/meld-$Target.zip"

$tmp = Join-Path $env:TEMP "meld-install-$(Get-Random)"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
try {
    Write-Host "==> Downloading the latest meld..."
    $zip = Join-Path $tmp "meld.zip"
    try {
        Invoke-WebRequest $AssetUrl -OutFile $zip
    } catch {
        Write-Error "No Windows build found in the latest release. Please report this at https://github.com/$Repo/issues"
    }

    Write-Host "==> Installing to $InstallDir"
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Expand-Archive -Path $zip -DestinationPath $InstallDir -Force

    # Add to the user PATH if not already there (takes effect in new terminals).
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
        Write-Host "==> Added meld to your PATH (open a new terminal to use it)"
    }

    Write-Host ""
    Write-Host "Meld installed. Get started with:"
    Write-Host "    meld doctor"
    Write-Host "    meld sync --dry-run"
    Write-Host "    meld sync"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
