param([Parameter(Mandatory=$true)][string]$Directory)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().User
$items = @(Get-Item -LiteralPath $Directory) + @(Get-ChildItem -LiteralPath $Directory -Force)
foreach ($item in $items) {
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Linked private state is refused' }
    if ($item.PSIsContainer -and $item.FullName -ne $items[0].FullName) { throw 'Unexpected directory in private state' }
    if ($item.GetAccessControl().GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $identity.Value) {
        throw 'Private development state must already belong to the current user'
    }
}
foreach ($item in $items) {
    if ($item.PSIsContainer) {
        $acl = New-Object Security.AccessControl.DirectorySecurity
        $rule = New-Object Security.AccessControl.FileSystemAccessRule($identity, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
    } else {
        $acl = New-Object Security.AccessControl.FileSecurity
        $rule = New-Object Security.AccessControl.FileSystemAccessRule($identity, 'FullControl', 'Allow')
    }
    # Preserve ownership: an owner can rewrite the DACL without WRITE_OWNER.
    # Setting even the same owner would require that separate permission.
    $acl.SetAccessRuleProtection($true, $false)
    $acl.AddAccessRule($rule)
    $item.SetAccessControl($acl)
    $verified = Get-Acl -LiteralPath $item.FullName
    if ($verified.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $identity.Value) { throw 'Private state ownership changed' }
    foreach ($access in $verified.Access) {
        if ($access.AccessControlType -eq 'Allow' -and $access.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value -ne $identity.Value) {
            throw 'Unexpected access to private development state'
        }
    }
}
