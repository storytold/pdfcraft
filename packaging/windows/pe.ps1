# Shared checks for the actual binaries shipped in Windows installers.
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}

function Assert-PeHeader([string] $Path, [int] $Machine, [int] $Subsystem) {
  $h = Get-PeHeader $Path
  if ($h.Machine -ne $Machine) { throw "$Path is for machine 0x$('{0:X}' -f $h.Machine), expected 0x$('{0:X}' -f $Machine)" }
  if ($h.Subsystem -ne $Subsystem) { throw "$Path has PE subsystem $($h.Subsystem), expected $Subsystem" }
  Write-Output "ok $Path`: PE subsystem $($h.Subsystem)"
}
