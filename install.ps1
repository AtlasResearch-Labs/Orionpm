# Orion installer for Windows (PowerShell)
# Built by Atlas Research Labs. Built for the World.

$ErrorActionPreference = "Stop"

$Repo = "AtlasResearch-Labs/Orionpm"
$InstallDir = "$env:USERPROFILE\.orion\bin"

Write-Host "Installing Orion process manager..."

$Archive = "orion-windows-x86_64.zip"
$Url = "https://github.com/$Repo/releases/latest/download/$Archive"

$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null
$ZipPath = Join-Path $TempDir $Archive

try {
    Write-Host "Downloading $Archive..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing

    Write-Host "Extracting to $InstallDir..."
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    tar -xf $ZipPath -C $InstallDir

    Write-Host "Orion installed successfully."

    # Check and update PATH in user environment
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        Write-Host "Adding $InstallDir to user PATH..."
        $NewPath = "$UserPath;$InstallDir"
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        $env:Path = "$env:Path;$InstallDir"
    }

    Write-Host "Run 'orion --help' to get started."
}
finally {
    Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}
