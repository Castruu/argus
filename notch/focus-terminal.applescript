-- Focus the Terminal.app tab running a session, by tty.
on run argv
	set theTty to item 1 of argv
	tell application "Terminal"
		repeat with w in windows
			repeat with t in tabs of w
				if tty of t is theTty then
					set selected of t to true
					set index of w to 1
				end if
			end repeat
		end repeat
		activate
	end tell
end run
