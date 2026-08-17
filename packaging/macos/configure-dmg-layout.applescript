on positionText(labelText, itemPosition)
	return labelText & "=" & (item 1 of itemPosition as text) & "," & (item 2 of itemPosition as text)
end positionText

on closeWindow(targetFolder)
	tell application "Finder"
		if exists container window of targetFolder then close container window of targetFolder
	end tell
end closeWindow

on run arguments
	if (count of arguments) is not 2 then error "DMG 布局脚本需要操作模式和挂载目录。"

	set operationName to item 1 of arguments
	set mountPath to item 2 of arguments
	set mountAlias to POSIX file mountPath as alias

	tell application "Finder"
		set targetFolder to folder mountAlias

		if operationName is "close" then
			my closeWindow(targetFolder)
			return "closed"
		end if

		open targetFolder
		set targetWindow to container window of targetFolder

		if operationName is "configure" then
			set hasTerminalCommand to exists item "终端启动命令.txt" of targetFolder
			-- Keep the primary app-to-Applications relationship from the v1.1.2 DMG.
			tell targetWindow
				set current view to icon view
				set toolbar visible to false
				set statusbar visible to false
				set pathbar visible to false
				set sidebar width to 0
				if hasTerminalCommand then
					set bounds to {100, 100, 1180, 700}
				else
					set bounds to {100, 100, 1020, 700}
				end if
			end tell
			set targetViewOptions to the icon view options of targetWindow
			tell targetViewOptions
				set arrangement to not arranged
				set icon size to 128
				set text size to 14
				set shows icon preview to true
			end tell
			update targetFolder without registering applications
			my closeWindow(targetFolder)
			set targetFolder to folder (POSIX file mountPath as alias)
			open targetFolder
			set targetWindow to container window of targetFolder
			set current view of targetWindow to icon view
			set targetViewOptions to icon view options of targetWindow
			set arrangement of targetViewOptions to not arranged
			set icon size of targetViewOptions to 128

			if hasTerminalCommand then
				set position of item "token-station.app" of targetFolder to {360, 170}
				set position of item "Applications" of targetFolder to {720, 170}
				set position of item "安装 Token Station.command" of targetFolder to {100, 440}
				set position of item "installation-guide.md" of targetFolder to {280, 440}
				set position of item "macos-troubleshooting.md" of targetFolder to {460, 440}
				if exists item "终端启动命令.txt" of targetFolder then set position of item "终端启动命令.txt" of targetFolder to {640, 440}
				if exists item "构建来源.txt" of targetFolder then set position of item "构建来源.txt" of targetFolder to {820, 440}
				if exists item "未签名测试版.txt" of targetFolder then set position of item "未签名测试版.txt" of targetFolder to {1000, 440}
			else
				set position of item "token-station.app" of targetFolder to {310, 170}
				set position of item "Applications" of targetFolder to {610, 170}
				set position of item "安装 Token Station.command" of targetFolder to {100, 440}
				set position of item "installation-guide.md" of targetFolder to {280, 440}
				set position of item "macos-troubleshooting.md" of targetFolder to {460, 440}
				if exists item "构建来源.txt" of targetFolder then set position of item "构建来源.txt" of targetFolder to {460, 440}
				if exists item "未签名测试版.txt" of targetFolder then set position of item "未签名测试版.txt" of targetFolder to {640, 440}
			end if

			update targetFolder without registering applications
			my closeWindow(targetFolder)
			return "configured"
		end if

		if operationName is "inspect" then
			set windowBounds to bounds of targetWindow
			set windowWidth to (item 3 of windowBounds) - (item 1 of windowBounds)
			set windowHeight to (item 4 of windowBounds) - (item 2 of windowBounds)
			set viewName to current view of targetWindow as text
			set iconSizeValue to icon size of icon view options of targetWindow
			set arrangementValue to arrangement of icon view options of targetWindow as text

			set resultLines to {¬
				"window=" & windowWidth & "x" & windowHeight, ¬
				"view=" & viewName, ¬
				"icon_size=" & iconSizeValue, ¬
				"arrangement=" & arrangementValue, ¬
				"toolbar=" & (toolbar visible of targetWindow as text), ¬
				"statusbar=" & (statusbar visible of targetWindow as text), ¬
				"pathbar=" & (pathbar visible of targetWindow as text), ¬
				"sidebar_width=" & sidebar width of targetWindow, ¬
				my positionText("app", position of item "token-station.app" of targetFolder), ¬
				my positionText("applications", position of item "Applications" of targetFolder), ¬
				my positionText("installer", position of item "安装 Token Station.command" of targetFolder), ¬
				my positionText("readme", position of item "installation-guide.md" of targetFolder), ¬
				my positionText("troubleshooting", position of item "macos-troubleshooting.md" of targetFolder)}
			if exists item "终端启动命令.txt" of targetFolder then set end of resultLines to my positionText("terminal_command", position of item "终端启动命令.txt" of targetFolder)
			if exists item "构建来源.txt" of targetFolder then set end of resultLines to my positionText("provenance", position of item "构建来源.txt" of targetFolder)
			if exists item "未签名测试版.txt" of targetFolder then set end of resultLines to my positionText("warning", position of item "未签名测试版.txt" of targetFolder)
			my closeWindow(targetFolder)
			set previousDelimiters to AppleScript's text item delimiters
			set AppleScript's text item delimiters to linefeed
			set resultText to resultLines as text
			set AppleScript's text item delimiters to previousDelimiters
			return resultText
		end if

		my closeWindow(targetFolder)
		error "不支持的 DMG 布局操作：" & operationName
	end tell
end run
