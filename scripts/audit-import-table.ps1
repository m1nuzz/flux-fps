<#
.SYNOPSIS
Audits the resident binary's PE import table for driver-loading and
process-injection APIs.

.DESCRIPTION
Hard invariant (AGENTS.md): flux-fps.exe must never contain driver-loading
code, game memory access or DLL injection. Kernel work belongs exclusively to
the portable flux-fps-master tool. This script parses the PE headers directly,
so it needs no dumpbin, objdump or extra CI dependency.

Fails with exit code 1 on any finding.
#>
[CmdletBinding()]
param(
    [string]$BinaryPath = 'target/release/flux-fps.exe'
)

$ErrorActionPreference = 'Stop'

$forbiddenImports = @(
    'NtLoadDriver', 'ZwLoadDriver', 'NtUnloadDriver', 'ZwUnloadDriver',
    'CreateServiceA', 'CreateServiceW', 'OpenSCManagerA', 'OpenSCManagerW',
    'StartServiceA', 'StartServiceW',
    'NtWriteVirtualMemory', 'ZwWriteVirtualMemory',
    'NtReadVirtualMemory', 'ZwReadVirtualMemory',
    'WriteProcessMemory', 'ReadProcessMemory', 'VirtualAllocEx',
    'CreateRemoteThread', 'CreateRemoteThreadEx',
    'NtCreateThreadEx', 'ZwCreateThreadEx',
    'SetWindowsHookExA', 'SetWindowsHookExW'
)

$forbiddenStrings = @(
    'NtLoadDriver', 'ZwLoadDriver', 'CreateServiceW', 'OpenSCManagerW',
    'WriteProcessMemory', 'CreateRemoteThread', '\??\'
)

function Get-U16 {
    param([byte[]]$Data, [int]$Offset)
    if ($Offset -lt 0 -or $Offset + 2 -gt $Data.Length) { throw "PE header truncated at $Offset" }
    return [BitConverter]::ToUInt16($Data, $Offset)
}

function Get-U32 {
    param([byte[]]$Data, [int]$Offset)
    if ($Offset -lt 0 -or $Offset + 4 -gt $Data.Length) { throw "PE header truncated at $Offset" }
    return [BitConverter]::ToUInt32($Data, $Offset)
}

function Get-I32 {
    param([byte[]]$Data, [int]$Offset)
    if ($Offset -lt 0 -or $Offset + 4 -gt $Data.Length) { throw "PE header truncated at $Offset" }
    return [BitConverter]::ToInt32($Data, $Offset)
}

function Get-AsciiZ {
    param([byte[]]$Data, [int]$Offset)
    if ($Offset -lt 0 -or $Offset -ge $Data.Length) { return '' }
    $end = $Offset
    while ($end -lt $Data.Length -and $Data[$end] -ne 0) { $end++ }
    return [System.Text.Encoding]::ASCII.GetString($Data, $Offset, $end - $Offset)
}

function Convert-RvaToOffset {
    param([byte[]]$Data, [uint32]$Rva, [int]$SectionStart, [int]$SectionCount)
    for ($i = 0; $i -lt $SectionCount; $i++) {
        $header = $SectionStart + ($i * 40)
        $virtualSize = Get-U32 $Data ($header + 8)
        $virtualAddress = Get-U32 $Data ($header + 12)
        $rawSize = Get-U32 $Data ($header + 16)
        $rawPointer = Get-U32 $Data ($header + 20)
        $span = [Math]::Max($virtualSize, $rawSize)
        if ($Rva -ge $virtualAddress -and $Rva -lt ($virtualAddress + $span)) {
            return [int]($rawPointer + ($Rva - $virtualAddress))
        }
    }
    return -1
}

$resolved = (Resolve-Path -LiteralPath $BinaryPath -ErrorAction Stop).Path
$bytes = [System.IO.File]::ReadAllBytes($resolved)

if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
    throw "Not a DOS/PE image: $resolved"
}
$peOffset = Get-I32 $bytes 0x3C
if ($peOffset -lt 0 -or $peOffset + 24 -gt $bytes.Length) { throw 'Invalid PE header offset' }
if ($bytes[$peOffset] -ne 0x50 -or $bytes[$peOffset + 1] -ne 0x45 -or
    $bytes[$peOffset + 2] -ne 0 -or $bytes[$peOffset + 3] -ne 0) {
    throw 'Missing PE\0\0 signature'
}

$sectionCount = Get-U16 $bytes ($peOffset + 6)
$optionalSize = Get-U16 $bytes ($peOffset + 20)
$optionalStart = $peOffset + 24
$magic = Get-U16 $bytes $optionalStart

if ($magic -eq 0x20B) {
    $dataDirectory = 112
    $thunkWidth = 8
    $ordinalBit = [uint64]0x8000000000000000
}
elseif ($magic -eq 0x10B) {
    $dataDirectory = 96
    $thunkWidth = 4
    $ordinalBit = [uint64]0x80000000
}
else {
    throw ("Unsupported optional header magic 0x{0:X}" -f $magic)
}

# Data directory entry 1 is the import table (RVA at +8, size at +12).
$importRva = Get-U32 $bytes ($optionalStart + $dataDirectory + 8)
$sectionStart = $optionalStart + $optionalSize

$importNames = New-Object 'System.Collections.Generic.List[string]'
$importDlls = New-Object 'System.Collections.Generic.List[string]'

if ($importRva -ne 0) {
    $descriptor = Convert-RvaToOffset $bytes $importRva $sectionStart $sectionCount
    if ($descriptor -lt 0) { throw 'Import directory RVA is outside every section' }

    for ($d = 0; $d -lt 4096; $d++) {
        $entry = $descriptor + ($d * 20)
        $originalThunk = Get-U32 $bytes ($entry + 0)
        $nameRva = Get-U32 $bytes ($entry + 12)
        $firstThunk = Get-U32 $bytes ($entry + 16)
        if ($nameRva -eq 0 -and $firstThunk -eq 0) { break }

        $dllOffset = Convert-RvaToOffset $bytes $nameRva $sectionStart $sectionCount
        if ($dllOffset -ge 0) {
            [void]$importDlls.Add((Get-AsciiZ $bytes $dllOffset))
        }

        $thunkRva = if ($originalThunk -ne 0) { $originalThunk } else { $firstThunk }
        $thunkOffset = Convert-RvaToOffset $bytes $thunkRva $sectionStart $sectionCount
        if ($thunkOffset -lt 0) { continue }

        for ($t = 0; $t -lt 4096; $t++) {
            $slot = $thunkOffset + ($t * $thunkWidth)
            if ($thunkWidth -eq 8) {
                if ($slot + 8 -gt $bytes.Length) { break }
                $value = [BitConverter]::ToUInt64($bytes, $slot)
            }
            else {
                $value = [uint64](Get-U32 $bytes $slot)
            }
            if ($value -eq 0) { break }
            if (($value -band $ordinalBit) -ne 0) { continue }

            $hintRva = [uint32]($value -band 0x7FFFFFFF)
            $hintOffset = Convert-RvaToOffset $bytes $hintRva $sectionStart $sectionCount
            if ($hintOffset -lt 0 -or $hintOffset + 2 -gt $bytes.Length) { continue }
            $name = Get-AsciiZ $bytes ($hintOffset + 2)
            if (-not [string]::IsNullOrEmpty($name)) { [void]$importNames.Add($name) }
        }
    }
}

$findings = @()
foreach ($name in $importNames) {
    if ($forbiddenImports -contains $name) { $findings += "forbidden import: $name" }
}
$asciiImage = [System.Text.Encoding]::ASCII.GetString($bytes)
foreach ($needle in $forbiddenStrings) {
    if ($asciiImage.Contains($needle)) { $findings += "forbidden string: $needle" }
}

Write-Output "Audited: $resolved"
Write-Output "Imported DLLs: $($importDlls.Count); imported functions: $($importNames.Count)"
if ($findings.Count -gt 0) {
    foreach ($finding in $findings) { Write-Output "FAIL: $finding" }
    exit 1
}
Write-Output 'PASS: no driver-loading or injection imports in the resident binary'
