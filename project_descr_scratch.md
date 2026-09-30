# General description
This is a notes taking mobile app.
It is security first, it shouldn't have fancy features, internet connection requirement, google services use, etc.
# Target device
It must run on Android and Graphene OS. I have Pixel 9a with GrapheneOS, I want my app to launch on it.
# Tech stack
- Pure Rust
- Slint
# How I will use it
I already have notes directory on my linux desktop.
I have tree of subdirs for different categories.
I edit it with neovim or other text editors.
I want to have the same notes on my phone available with on-demand sync via bluetooth or other form of wireless connection, the sync must be end-to-end ecrypted with static keys created on each device once.
I want any note created on my device be available with any text editor on any device.
Notes must be markdown only.
Notes must be stored as plain files in directory-tree.
# Notes storage
Notes are plain files in directory tree.
App should allow to create multiple independent vaults, each vault is top-node directory.
# Navigation and control
- User can add new directories in any directory in the vault
- User can add new notes in any directory of the vault
- User can delete any directory and any note in any directory in the vault with confirmation dialog
- User can navigate through directories tree by tapping on directory box or by pressing back arrow
# UI style
- dark theme
- minimalistic
- take inspiration for colours from ghostty theme called "Cyberpunk Scarlet Protocol" (search on internet)
# Note editing and appearance
- Note should be rendered as markdown.
- The line where carriage stands currently must show plain text and remove markdown rendering so it is easy to edit it.
- As soon as carriage leaves the line it is rendered as markdown.
- When user leaves note, it is automatically saved
## Special features
App, when in editing mode, should have button to add the taskbox element fast `- [ ] ` and then stop to allow user to add description of task.
# Sync
- Sync of each vault should be independent from others
- Sync can be initiated from both mobile app and linux desktop (I'll also need small cli utility for desktop)
- Sync must be end-to-end encrypted
