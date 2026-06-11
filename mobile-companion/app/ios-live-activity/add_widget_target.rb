#!/usr/bin/env ruby
# Internal/dev signing tooling — set APPLE_* env vars to use your own Apple Developer account.
# Idempotently adds the FurxWidgets Live Activity app-extension target to the
# Tauri-generated Xcode project (src-tauri/gen/apple/app.xcodeproj). Re-runnable
# after `tauri ios init` (council LA1). Source files live in this versioned dir,
# OUTSIDE gen/apple (which is regenerated), and are referenced by path.
#
# Usage: ruby add_widget_target.rb
require "xcodeproj"

HERE = File.expand_path(File.dirname(__FILE__))                      # .../app/ios-live-activity
PROJECT = File.join(HERE, "..", "src-tauri", "gen", "apple", "app.xcodeproj")
APP_TARGET = "app_iOS"
EXT_NAME = "FurxWidgets"
EXT_BUNDLE_ID = "cloud.furx.companion.FurxWidgets"
TEAM = ENV.fetch("APPLE_TEAM_ID")  # set APPLE_TEAM_ID to your Apple Developer Team ID
PROFILE = "Furx Widgets App Store"
EXT_SOURCES = ["FurxActivityAttributes.swift", "FurxWidgetsBundle.swift"]
EXT_PLIST = File.join(HERE, "FurxWidgets-Info.plist")

abort("project not found: #{PROJECT} (run `tauri ios init` first)") unless Dir.exist?(PROJECT)
project = Xcodeproj::Project.open(PROJECT)

app = project.targets.find { |t| t.name == APP_TARGET }
abort("app target #{APP_TARGET} not found") unless app

if project.targets.any? { |t| t.name == EXT_NAME }
  puts "FurxWidgets target already present — nothing to do (idempotent)."
  exit 0
end

# 1. New app-extension target (iOS 16.1 — Live Activity minimum).
ext = project.new_target(:app_extension, EXT_NAME, :ios, "16.1", nil, :swift)

# 2. Source files + Info.plist (referenced by path from this versioned dir).
group = project.main_group.new_group(EXT_NAME, HERE)
EXT_SOURCES.each do |name|
  ref = group.new_reference(File.join(HERE, name))
  ext.add_file_references([ref])
end

# 3. Build settings.
ext.build_configurations.each do |c|
  bs = c.build_settings
  bs["PRODUCT_BUNDLE_IDENTIFIER"] = EXT_BUNDLE_ID
  bs["PRODUCT_NAME"] = EXT_NAME
  bs["INFOPLIST_FILE"] = EXT_PLIST
  bs["IPHONEOS_DEPLOYMENT_TARGET"] = "16.1"
  bs["SWIFT_VERSION"] = "5.0"
  bs["TARGETED_DEVICE_FAMILY"] = "1,2"
  bs["SKIP_INSTALL"] = "YES"            # extensions are embedded, not installed standalone
  bs["GENERATE_INFOPLIST_FILE"] = "NO"
  bs["MARKETING_VERSION"] = "0.1.0"
  bs["CURRENT_PROJECT_VERSION"] = "1"
  bs["DEVELOPMENT_TEAM"] = TEAM
  bs["CODE_SIGN_STYLE"] = "Manual"
  bs["CODE_SIGN_IDENTITY"] = "iPhone Distribution"
  bs["PROVISIONING_PROFILE_SPECIFIER"] = PROFILE
  bs["LD_RUNPATH_SEARCH_PATHS"] = ["$(inherited)", "@executable_path/Frameworks", "@executable_path/../../Frameworks"]
end

# 4. Embed the .appex into the app's PlugIns, code-signed on copy.
embed = app.new_copy_files_build_phase("Embed App Extensions")
embed.symbol_dst_subfolder_spec = :plug_ins
bf = embed.add_file_reference(ext.product_reference)
bf.settings = { "ATTRIBUTES" => ["RemoveHeadersOnCopy"] }

# 5. App depends on the extension (build order).
app.add_dependency(ext)

project.save
puts "Added #{EXT_NAME} extension target (bundle #{EXT_BUNDLE_ID}), embedded into #{APP_TARGET}."
