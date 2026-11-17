-- Focus the Ghostty terminal running a session: by tty (Claude) or working directory (Codex).
on run argv
	set {theTty, theCwd} to {item 1 of argv, item 2 of argv}
	tell application "Ghostty"
		if theTty is not "" then
			set matches to (every terminal whose tty is theTty)
		else
			set matches to (every terminal whose working directory is theCwd)
		end if
		if (count of matches) > 0 then focus (item 1 of matches)
		activate
	end tell
end run
