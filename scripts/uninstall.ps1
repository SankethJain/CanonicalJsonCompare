# Removes Mongo Compare from this PC.
#
# Usually started from Settings > Apps > Installed apps > Mongo Compare > Uninstall.
# It can also be run directly: right-click uninstall.ps1 > Run with PowerShell.
#
# Your compare reports (compare-report-* folders) and export files are not touched.

$ErrorActionPreference = 'Stop'
$Bin = 'mongo-compare'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\mongo-compare'

# Leave the program folder so it can be deleted.
Set-Location $env:TEMP

$running = Get-Process -Name $Bin -ErrorAction SilentlyContinue
if ($running) {
    Write-Host 'Closing Mongo Compare...'
    $running | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
}

foreach ($folder in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {
    Remove-Item (Join-Path $folder 'Mongo Compare.lnk') -Force -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -and (($userPath -split ';') -contains $dir)) {
    $kept = ($userPath -split ';') | Where-Object { $_ -and $_ -ne $dir }
    [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
}

# Remembered settings and the sample files made by Ctrl+D / --demo.
Remove-Item (Join-Path $env:APPDATA $Bin) -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item (Join-Path $env:TEMP 'mongo-compare-demo') -Recurse -Force -ErrorAction SilentlyContinue

# The entry in Settings > Apps.
Remove-Item 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\mongo-compare' -Recurse -Force -ErrorAction SilentlyContinue

Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
if (Test-Path $dir) {
    Write-Host "Some files could not be removed. Close any window using them and delete this folder: $dir"
} else {
    Write-Host 'Mongo Compare has been removed.'
}

# Started as a file (from Settings or "Run with PowerShell"): keep the window
# open so the result can be read.
if ($PSCommandPath) {
    Read-Host 'Press Enter to close'
}
