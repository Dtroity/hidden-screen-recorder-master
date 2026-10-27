' Silent launcher for Watch-Agent.ps1 — Task Scheduler must not flash cmd.exe.
Option Explicit
Dim sh, fso, root, ps1, cmd
Set sh = CreateObject("WScript.Shell")
Set fso = CreateObject("Scripting.FileSystemObject")
root = fso.GetParentFolderName(WScript.ScriptFullName)
ps1 = root & "\Watch-Agent.ps1"
If Not fso.FileExists(ps1) Then WScript.Quit 0
cmd = "powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File """ & ps1 & """"
' 0 = hidden window, False = do not wait
sh.Run cmd, 0, False
