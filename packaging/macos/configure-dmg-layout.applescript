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
		set backgroundFile to file ".background:background.png" of targetFolder

		if operationName is "close" then
			my closeWindow(targetFolder)
			return "closed"
		end if

		open targetFolder
		set targetWindow to container window of targetFolder

		if operationName is "configure" then
			-- Keep one primary drag relationship and one visible guide.
			tell targetWindow
				set current view to icon view
				set toolbar visible to false
				set statusbar visible to false
				set pathbar visible to false
				set sidebar width to 0
				set bounds to {100, 100, 1280, 740}
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
			set background picture of targetViewOptions to backgroundFile

			-- Finder stores item positions relative to the content area, then reports them 45 px lower
			-- after the title bar is restored. These write coordinates yield the audited positions below.
			set position of item "token-station.app" of targetFolder to {300, 240}
			set position of item "Applications" of targetFolder to {880, 240}
			set position of item "README.md" of targetFolder to {590, 455}

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
				my positionText("readme", position of item "README.md" of targetFolder)}
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
