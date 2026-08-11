on positionText(labelText, itemPosition)
	return labelText & "=" & (item 1 of itemPosition as text) & "," & (item 2 of itemPosition as text)
end positionText

on closeWindow(targetDisk)
	tell application "Finder"
		if exists container window of targetDisk then close container window of targetDisk
	end tell
end closeWindow

on run arguments
	if (count of arguments) is not 2 then error "DMG 布局脚本需要操作模式和挂载目录。"

	set operationName to item 1 of arguments
	set mountPath to item 2 of arguments
	set mountAlias to POSIX file mountPath as alias

	tell application "Finder"
		set targetDisk to disk of mountAlias

		if operationName is "close" then
			my closeWindow(targetDisk)
			return "closed"
		end if

		open targetDisk
		set targetWindow to container window of targetDisk

		if operationName is "configure" then
			-- Keep the primary app-to-Applications relationship from the v1.1.2 DMG.
			tell targetWindow
				set current view to icon view
				set toolbar visible to false
				set statusbar visible to false
				set pathbar visible to false
				set sidebar width to 0
				set bounds to {100, 100, 1020, 700}
			end tell
			set targetViewOptions to the icon view options of targetWindow
			tell targetViewOptions
				set arrangement to not arranged
				set icon size to 128
				set text size to 14
				set shows icon preview to true
			end tell
			update targetDisk without registering applications
			my closeWindow(targetDisk)
			set targetDisk to disk of (POSIX file mountPath as alias)
			open targetDisk
			set targetWindow to container window of targetDisk

			set position of item "token-station.app" of targetDisk to {310, 170}
			set position of item "Applications" of targetDisk to {610, 170}
			set position of item "安装 Token Station.command" of targetDisk to {100, 440}
			set position of item "安装前必读.md" of targetDisk to {280, 440}
			if exists item "构建来源.txt" of targetDisk then set position of item "构建来源.txt" of targetDisk to {460, 440}
			if exists item "未签名测试版.txt" of targetDisk then set position of item "未签名测试版.txt" of targetDisk to {640, 440}
			set position of item "AGENTS.md" of targetDisk to {820, 440}

			update targetDisk without registering applications
			my closeWindow(targetDisk)
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
				my positionText("app", position of item "token-station.app" of targetDisk), ¬
				my positionText("applications", position of item "Applications" of targetDisk), ¬
				my positionText("installer", position of item "安装 Token Station.command" of targetDisk), ¬
				my positionText("readme", position of item "安装前必读.md" of targetDisk)}
			if exists item "构建来源.txt" of targetDisk then set end of resultLines to my positionText("provenance", position of item "构建来源.txt" of targetDisk)
			if exists item "未签名测试版.txt" of targetDisk then set end of resultLines to my positionText("warning", position of item "未签名测试版.txt" of targetDisk)
			set end of resultLines to my positionText("agent_rules", position of item "AGENTS.md" of targetDisk)

			my closeWindow(targetDisk)
			set previousDelimiters to AppleScript's text item delimiters
			set AppleScript's text item delimiters to linefeed
			set resultText to resultLines as text
			set AppleScript's text item delimiters to previousDelimiters
			return resultText
		end if

		my closeWindow(targetDisk)
		error "不支持的 DMG 布局操作：" & operationName
	end tell
end run
