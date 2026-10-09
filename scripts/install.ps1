# Installs mongo-compare on Windows from the latest GitHub release.
#
# Run this in PowerShell (no administrator rights needed):
#   irm https://github.com/SankethJain/CanonicalJsonCompare/releases/latest/download/install.ps1 | iex
#
# It puts the program in %LOCALAPPDATA%\Programs\mongo-compare, adds it to
# your PATH, creates "Mongo Compare" shortcuts in the Start menu and on the
# desktop, and lists it in Settings > Apps so it can be uninstalled there.

$ErrorActionPreference = 'Stop'
$Repo = 'SankethJain/CanonicalJsonCompare'
$Bin = 'mongo-compare'
$Version = if ($env:MONGO_COMPARE_VERSION) { $env:MONGO_COMPARE_VERSION } else { 'latest' }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
$target = "$arch-pc-windows-msvc"
$base = if ($Version -eq 'latest') {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}
$url = "$base/$Bin-$target.zip"
$dir = Join-Path $env:LOCALAPPDATA 'Programs\mongo-compare'
$zip = Join-Path $env:TEMP "$Bin-$([guid]::NewGuid()).zip"

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Write-Host "Downloading $Bin ($target)..."
Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing

$sums = $null
try {
    $sums = (Invoke-WebRequest -Uri "$url.sha256" -UseBasicParsing).Content
} catch {
    # No checksum published for this release; continue without it.
}
if ($sums) {
    if ($sums -is [byte[]]) { $sums = [Text.Encoding]::ASCII.GetString($sums) }
    $expected = ($sums.Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        Remove-Item $zip
        throw 'Checksum mismatch, the download may be damaged. Please try again.'
    }
}

New-Item -ItemType Directory -Force $dir | Out-Null
Expand-Archive -Path $zip -DestinationPath $dir -Force
Remove-Item $zip
$exe = Join-Path $dir "$Bin.exe"
# Remove the "downloaded from the internet" mark so Windows SmartScreen does
# not block the program when it is started from the shortcut.
Get-ChildItem $dir -Recurse | Unblock-File
Write-Host "Installed to $exe"

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $userPath) { $userPath = '' }
if (-not (($userPath -split ';') -contains $dir)) {
    $newPath = if ($userPath) { "$userPath;$dir" } else { $dir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "Added $dir to your PATH (open a new terminal to use it)."
}

$shell = New-Object -ComObject WScript.Shell
foreach ($folder in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {
    $lnk = $shell.CreateShortcut((Join-Path $folder 'Mongo Compare.lnk'))
    $lnk.TargetPath = $exe
    $lnk.WorkingDirectory = [Environment]::GetFolderPath('MyDocuments')
    $lnk.Description = 'Compare two MongoDB exports'
    $lnk.Save()
}

# Add "Mongo Compare" to Settings > Apps, so it can be uninstalled from there.
$uninstaller = Join-Path $dir 'uninstall.ps1'
if (Test-Path $uninstaller) {
    $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\mongo-compare'
    $version = ''
    try { $version = ((& $exe --version) -split ' ')[-1] } catch { }
    New-Item -Path $key -Force | Out-Null
    $values = @{
        DisplayName     = 'Mongo Compare'
        DisplayVersion  = $version
        DisplayIcon     = $exe
        InstallLocation = $dir
        UninstallString = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$uninstaller`""
    }
    foreach ($name in $values.Keys) {
        New-ItemProperty -Path $key -Name $name -Value $values[$name] -PropertyType String -Force | Out-Null
    }
    foreach ($name in @('NoModify', 'NoRepair')) {
        New-ItemProperty -Path $key -Name $name -Value 1 -PropertyType DWord -Force | Out-Null
    }
}

Write-Host ''
Write-Host 'Done! Open "Mongo Compare" from the Start menu or your desktop,'
Write-Host "or type $Bin in a new terminal window."
