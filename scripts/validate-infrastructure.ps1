<#
.SYNOPSIS
CI gate for the installer infrastructure: PowerShell syntax, XML
well-formedness and the required winget manifest set.

.DESCRIPTION
Runs on windows-latest without any extra tooling. Fails with exit code 1 on
the first category of problems found, listing every issue first.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$problems = New-Object 'System.Collections.Generic.List[string]'

$scriptDir = Join-Path $root 'scripts'
$scriptFiles = Get-ChildItem -LiteralPath $scriptDir -Filter '*.ps1' -File
foreach ($scriptFile in $scriptFiles) {
    $tokens = $null
    $parseErrors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile(
        $scriptFile.FullName, [ref]$tokens, [ref]$parseErrors)
    foreach ($parseError in $parseErrors) {
        [void]$problems.Add("$($scriptFile.Name): $($parseError.Message)")
    }
}
Write-Output "Parsed $($scriptFiles.Count) PowerShell script(s)."

$installerDir = Join-Path $root 'installer'
$xmlFiles = @()
if (Test-Path -LiteralPath $installerDir) {
    $xmlFiles = @(Get-ChildItem -LiteralPath $installerDir -Recurse -File |
        Where-Object { $_.Extension -in @('.wxs', '.wixproj', '.manifest') })
}
foreach ($xmlFile in $xmlFiles) {
    try {
        [xml]$document = Get-Content -LiteralPath $xmlFile.FullName -Raw
        if ($null -eq $document.DocumentElement) {
            [void]$problems.Add("$($xmlFile.Name): empty XML document")
        }
    }
    catch {
        [void]$problems.Add("$($xmlFile.Name): $($_.Exception.Message)")
    }
}
Write-Output "Parsed $($xmlFiles.Count) installer XML file(s)."

$productSource = Join-Path $installerDir 'Product.wxs'
if (-not (Test-Path -LiteralPath $productSource -PathType Leaf)) {
    [void]$problems.Add('missing MSI source: installer/Product.wxs')
}
else {
    $productText = Get-Content -LiteralPath $productSource -Raw
    if ($productText -notmatch 'Register-FluxFpsTask\.ps1' -or
        $productText -notmatch 'Unregister-FluxFpsTask\.ps1') {
        [void]$problems.Add('Product.wxs must install both Task Scheduler scripts')
    }
    if ($productText -notmatch 'UnregisterTasks') {
        [void]$problems.Add('Product.wxs must restore isolation state on uninstall')
    }
}

$wingetDir = Join-Path $root 'packaging/winget'
$expectedManifests = @(
    'm1nuzz.flux-fps.yaml',
    'm1nuzz.flux-fps.installer.yaml',
    'm1nuzz.flux-fps.locale.en-US.yaml'
)
foreach ($manifest in $expectedManifests) {
    $path = Join-Path $wingetDir $manifest
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        [void]$problems.Add("missing winget manifest: packaging/winget/$manifest")
        continue
    }
    $text = Get-Content -LiteralPath $path -Raw
    if ($text -notmatch 'PackageIdentifier:') {
        [void]$problems.Add("$manifest: missing PackageIdentifier")
    }
    if ($text -notmatch 'PackageVersion:') {
        [void]$problems.Add("$manifest: missing PackageVersion")
    }
    if ($text -notmatch 'ManifestVersion:') {
        [void]$problems.Add("$manifest: missing ManifestVersion")
    }
    if ($manifest -like '*.installer.yaml' -and
        ($text -notmatch 'InstallerUrl:' -or $text -notmatch 'InstallerSha256:')) {
        [void]$problems.Add("$manifest: missing InstallerUrl or InstallerSha256")
    }
}
Write-Output "Checked $($expectedManifests.Count) winget manifest(s)."

if ($problems.Count -gt 0) {
    foreach ($problem in $problems) { Write-Output "FAIL: $problem" }
    exit 1
}
Write-Output 'PASS: installer scripts, XML and winget manifests are well formed'
