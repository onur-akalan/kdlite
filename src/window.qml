// kdlite yonetim penceresi. kdlite surecinin yerel API'siyle (127.0.0.1) konusur.
// "qml6" ile calistirilir; @PORT@ baslatilirken gercek port numarasiyla degistirilir.
import QtQuick
import QtQuick.Controls as QQC
import QtQuick.Layouts
import QtQuick.Dialogs
import org.kde.kirigami as Kirigami

Window {
    id: root
    visible: true
    width: 720
    height: 600
    minimumWidth: 520
    minimumHeight: 420
    title: "kdlite"

    readonly property string base: "http://127.0.0.1:@PORT@"
    // Ceviri: kaynak dil Ingilizce; baska diller lang/<kod>.json dosyasindan
    // gelir ve pencere baslatilirken buraya gomulur.
    readonly property string lang: "@LANG@"
    readonly property var strings: (@STRINGS@)
    function t(s) { var v = strings[s]; return v === undefined ? s : v }
    property var st: null          // sunucudan gelen son durum
    property bool reachable: true
    property string errorText: ""
    property string pendingRemove: ""
    property string view: "main"   // "main" ya da "settings"
    property string pendingFree: ""      // "yalnizca bulutta" onayi bekleyen yedek
    property string pendingFreePath: ""
    property int noticeSeen: 0     // kapatilan son konum bildiriminin zamani
    function place(path, mode) {
        api("/api/place", { paths: [path], mode: mode }, function() { root.errorText = ""; root.refresh() })
    }
    readonly property var notice: st && st.notice && st.notice.at > noticeSeen ? st.notice : null

    function human(n) {
        var u = ["B", "KB", "MB", "GB", "TB"], i = 0
        while (n >= 1024 && i < u.length - 1) { n /= 1024; i++ }
        return (i === 0 ? n : n.toFixed(1)) + " " + u[i]
    }
    function ago(ts) {
        var s = Math.max(0, Math.floor(Date.now() / 1000) - ts)
        if (s < 60) return root.t("just now")
        if (s < 3600) return Math.floor(s / 60) + root.t(" min ago")
        if (s < 86400) return Math.floor(s / 3600) + root.t(" h ago")
        return Math.floor(s / 86400) + root.t(" d ago")
    }
    function shortPath(p) {
        return st && p.indexOf(st.home) === 0 ? "~" + p.slice(st.home.length) : p
    }
    function api(path, body, done) {
        var x = new XMLHttpRequest()
        x.onreadystatechange = function() {
            if (x.readyState !== XMLHttpRequest.DONE) return
            var j = null
            try { j = JSON.parse(x.responseText) } catch (e) {}
            if (x.status === 200) {
                root.reachable = true
                if (done) done(j)
            } else if (x.status === 0) {
                root.reachable = false
            } else {
                root.errorText = (j && j.error) ? j.error : root.t("Request failed")
            }
        }
        x.open(body === undefined ? "GET" : "POST", base + path)
        if (body !== undefined) {
            x.setRequestHeader("Content-Type", "application/json")
            x.setRequestHeader("X-Kdlite", "1")
            x.send(JSON.stringify(body))
        } else {
            x.send()
        }
    }
    function refresh() { api("/api/state", undefined, function(j) { root.st = j }) }

    readonly property bool setup: st !== null && st.setup === true   // ilk kurulum kipi
    readonly property bool running: st !== null && st.running === true
    readonly property bool paused: st !== null && st.paused
    // Durum rozeti: tepsi simgesindekiyle ayni semboller.
    readonly property string badgeIcon: !reachable ? "state-offline" : running ? "state-sync" : paused ? "emblem-pause"
        : anyFailed ? "emblem-warning" : sources.length > 0 ? "emblem-checked" : ""
    readonly property var sources: st && st.sources ? st.sources : []
    readonly property bool anyFailed: {
        for (var i = 0; i < sources.length; i++) {
            var l = sources[i].last
            if (!sources[i].exists || (l && (l.error || (l.stats && l.stats.failed > 0)))) return true
        }
        return false
    }
    readonly property string statusTitle: {
        if (!reachable) return root.t("kdlite is not running")
        if (!st) return root.t("Loading…")
        if (running && st.freeing) return root.t("Changing where files are kept")
        if (running) {
            var p = st.progress
            return p.total_files > 0 ? root.t("Backing up: ") + p.source + " · " + p.done_files + "/" + p.total_files + root.t(" files")
                                     : root.t("Scanning: ") + (p.source || "…")
        }
        if (paused) return root.t("Backup paused")
        if (sources.length === 0) return root.t("No folders selected yet")
        if (anyFailed) return root.t("Some files could not be backed up")
        for (var i = 0; i < sources.length; i++) if (!sources[i].last) return root.t("Waiting to back up")
        return root.t("Everything is backed up")
    }
    readonly property string statusDetail: {
        if (!reachable) return root.t("Restart kdlite from the application menu.")
        if (!st) return ""
        if (running && st.freeing) return st.progress.done_files + "/" + st.progress.total_files + root.t(" files")
        if (running) {
            var p = st.progress
            return p.total_files > 0 ? human(p.done_bytes) + " / " + human(p.total_bytes) + " · " + p.current : ""
        }
        if (paused) return root.t("Nothing is uploaded until you press “Resume”.")
        if (sources.length === 0) return root.t("Add the folders you want to back up; changes flow to kDrive on their own.")
        if (st.mount_failed > 0) return root.t("Saved to the kDrive folder: ") + st.mount_failed + root.t(" files could not be uploaded; will retry.")
        if (st.mount_pending > 0) return root.t("Saved to the kDrive folder: ") + st.mount_pending + root.t(" files uploading…")
        return st.watching ? root.t("Folders are watched; changes upload within seconds.")
                           : root.t("Changes are checked and uploaded every ") + Math.max(1, Math.round(st.interval / 60)) + root.t(" min.")
    }

    Timer {
        interval: root.running ? 1000 : 4000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    FolderDialog {
        id: picker
        title: root.t("Choose a folder to back up")
        onAccepted: {
            destDialog.localPath = decodeURIComponent(selectedFolder.toString().replace(/^file:\/\//, ""))
            destDialog.editName = ""
            destDialog.stack = []
            destDialog.load()
            destDialog.open()
        }
    }

    // Buluttaki hedef: klasor kDrive'da nereye yedeklensin?
    QQC.Dialog {
        id: destDialog
        property string localPath: ""
        property string editName: ""   // doluysa: var olan yedegin yeri degistiriliyor
        property var stack: []     // girilen bulut klasorleri: [{id, name}]
        property var dirs: []
        property bool loading: false
        readonly property string folderName: editName ? editName : localPath.replace(/\/+$/, "").split("/").pop()
        readonly property string here: stack.map(function(d) { return d.name }).join("/")
        function load() {
            loading = true
            var id = stack.length > 0 ? "?id=" + stack[stack.length - 1].id : ""
            root.api("/api/cloud" + id, undefined, function(j) { destDialog.dirs = j.dirs; destDialog.loading = false })
        }
        function add(dest) {
            var body = editName ? { name: editName } : { path: localPath }
            if (dest !== undefined) body.dest = dest
            root.api(editName ? "/api/edit" : "/api/add", body, function() { root.errorText = ""; root.refresh() })
            close()
        }
        title: root.t("Where in kDrive should it go?")
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 2, Kirigami.Units.gridUnit * 30)
        height: Math.min(root.height - Kirigami.Units.gridUnit * 2, Kirigami.Units.gridUnit * 26)
        contentItem: ColumnLayout {
            spacing: Kirigami.Units.smallSpacing
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: "“" + destDialog.folderName + root.t("”: choose its place in the cloud. The folder is created inside the place you pick, under its own name.")
                      + (destDialog.editName ? root.t(" The copy at the old place is not deleted; files are uploaded again to the new place.") : "")
            }
            RowLayout {
                Layout.fillWidth: true
                QQC.ToolButton {
                    icon.name: "go-up"
                    text: root.t("Up")
                    display: QQC.AbstractButton.IconOnly
                    enabled: destDialog.stack.length > 0
                    onClicked: { destDialog.stack = destDialog.stack.slice(0, -1); destDialog.load() }
                }
                Kirigami.Icon { source: "folder-cloud"; implicitWidth: Kirigami.Units.iconSizes.small; implicitHeight: implicitWidth }
                QQC.Label {
                    Layout.fillWidth: true
                    text: "kDrive" + (destDialog.here ? " / " + destDialog.stack.map(function(d) { return d.name }).join(" / ") : "")
                    font.bold: true
                    elide: Text.ElideLeft
                }
            }
            QQC.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                QQC.ScrollBar.horizontal.policy: QQC.ScrollBar.AlwaysOff
                ListView {
                    id: destList
                    clip: true
                    model: destDialog.dirs
                    QQC.Label {
                        anchors.centerIn: parent
                        visible: destList.count === 0
                        opacity: 0.6
                        text: destDialog.loading ? root.t("Loading…") : root.t("No subfolders")
                    }
                    delegate: QQC.ItemDelegate {
                        required property var modelData
                        width: ListView.view.width
                        icon.name: "folder"
                        text: modelData.name
                        onClicked: {
                            destDialog.stack = destDialog.stack.concat([{ id: modelData.id, name: modelData.name }])
                            destDialog.load()
                        }
                    }
                }
            }
            QQC.Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: root.t("Destination: ") + (destDialog.here ? destDialog.here + "/" : "") + destDialog.folderName
            }
        }
        footer: QQC.DialogButtonBox {
            QQC.Button {
                text: root.t("Default place (") + (root.st ? root.st.backup_root : "") + ")"
                QQC.DialogButtonBox.buttonRole: QQC.DialogButtonBox.ActionRole
                onClicked: destDialog.add(undefined)
            }
            QQC.Button {
                text: root.t("Back up here")
                icon.name: "dialog-ok-apply"
                QQC.DialogButtonBox.buttonRole: QQC.DialogButtonBox.AcceptRole
                onClicked: destDialog.add(destDialog.here)
            }
            QQC.Button {
                text: root.t("Cancel")
                QQC.DialogButtonBox.buttonRole: QQC.DialogButtonBox.RejectRole
                onClicked: destDialog.close()
            }
        }
    }

    QQC.Dialog {
        id: confirmRemove
        title: root.t("Remove from list")
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 24)
        standardButtons: QQC.Dialog.Ok | QQC.Dialog.Cancel
        QQC.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "“" + root.pendingRemove + root.t("” will no longer be backed up. The cloud copy is not deleted.")
        }
        onAccepted: root.api("/api/remove", { name: root.pendingRemove }, function() { root.refresh() })
    }

    QQC.Dialog {
        id: confirmLogout
        title: root.t("Sign out")
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
        standardButtons: QQC.Dialog.Ok | QQC.Dialog.Cancel
        QQC.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: root.t("The API token is removed from this computer and the kDrive folder is disconnected. ")
                + root.t("Your backup list, settings and cloud files stay as they are.\n\n")
                + root.t("The setup wizard opens next; you can connect again with the same or another token.")
        }
        onAccepted: { root.view = "main"; root.api("/api/logout", {}, function() {}) }
    }

    QQC.Dialog {
        id: confirmFree
        title: root.t("Keep in cloud only")
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
        standardButtons: QQC.Dialog.Ok | QQC.Dialog.Cancel
        QQC.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "“" + root.pendingFree + root.t("”: its files are removed from this computer and stay only in the cloud. ")
                + root.t("A shortcut stays in place of each: opening it downloads the file from the cloud.\n\n")
                + root.t("Only files verified right now to have an identical copy in the cloud are removed; the rest stay. ")
                + root.t("Removed files go to the trash; disk space is freed when you empty it.")
        }
        onAccepted: root.place(root.pendingFreePath, "cloud")
    }

    // Genel ayarlar sayfasi. Kaydedince servis yeni ayarlarla yeniden baslar.
    QQC.Page {
        id: settingsDialog
        anchors.fill: parent
        visible: !root.setup && root.view === "settings"
        property var cfg: null
        function load() {
            cfg = null
            root.api("/api/settings", undefined, function(j) {
                settingsDialog.cfg = j
                var langs = [{ code: "auto", name: root.t("Automatic (system language)") }, { code: "en", name: "English" }]
                var names = { tr: "Türkçe" }
                for (var i = 0; i < j.languages.length; i++)
                    langs.push({ code: j.languages[i], name: names[j.languages[i]] || j.languages[i] })
                langBox.model = langs
                langBox.currentIndex = 0
                for (var k = 0; k < langs.length; k++) if (langs[k].code === j.language) langBox.currentIndex = k
                writableSwitch.checked = j.writable
                deletesSwitch.checked = j.sync_deletes
                autostartSwitch.checked = j.autostart
                cacheSpin.value = Math.round(j.cache_max_gb)
                rescanSpin.value = j.rescan_min
                rootField.text = j.backup_root
            })
            root.view = "settings"
        }
        function save() {
            root.api("/api/settings", {
                language: langBox.model[langBox.currentIndex].code,
                writable: writableSwitch.checked,
                sync_deletes: deletesSwitch.checked,
                autostart: autostartSwitch.checked,
                cache_max_gb: cacheSpin.value,
                rescan_min: rescanSpin.value,
                backup_root: rootField.text
            }, function() { root.errorText = ""; root.view = "main"; root.refresh() })
        }
        header: QQC.ToolBar {
            contentItem: RowLayout {
                spacing: Kirigami.Units.largeSpacing
                QQC.ToolButton {
                    icon.name: "go-previous"
                    text: root.t("Back")
                    onClicked: root.view = "main"
                }
                Kirigami.Heading {
                    Layout.fillWidth: true
                    level: 3
                    text: root.t("Settings")
                }
                QQC.Button {
                    text: root.t("Save")
                    icon.name: "document-save"
                    highlighted: true
                    enabled: settingsDialog.cfg !== null
                    onClicked: settingsDialog.save()
                }
            }
        }
        Kirigami.InlineMessage {
            id: settingsError
            anchors { top: parent.top; left: parent.left; right: parent.right; margins: Kirigami.Units.smallSpacing }
            type: Kirigami.MessageType.Error
            text: root.errorText
            visible: root.view === "settings" && root.errorText.length > 0
            showCloseButton: true
            onVisibleChanged: if (!visible && root.view === "settings") root.errorText = ""
        }
        QQC.ScrollView {
            id: settingsScroll
            anchors { top: settingsError.visible ? settingsError.bottom : parent.top; left: parent.left; right: parent.right; bottom: parent.bottom }
            anchors.topMargin: Kirigami.Units.largeSpacing
            contentWidth: availableWidth
          Kirigami.FormLayout {
            x: Kirigami.Units.gridUnit
            width: settingsScroll.availableWidth - Kirigami.Units.gridUnit * 2
            enabled: settingsDialog.cfg !== null
            RowLayout {
                Kirigami.FormData.label: root.t("Account:")
                spacing: Kirigami.Units.largeSpacing
                Kirigami.Icon { source: "folder-cloud"; implicitWidth: Kirigami.Units.iconSizes.small; implicitHeight: implicitWidth }
                QQC.Label { text: settingsDialog.cfg ? settingsDialog.cfg.drive : "" }
                QQC.Button {
                    text: root.t("Sign out…")
                    icon.name: "system-log-out"
                    onClicked: confirmLogout.open()
                }
            }
            QQC.Switch {
                id: autostartSwitch
                Kirigami.FormData.label: root.t("Startup:")
                text: root.t("Start automatically at login")
            }
            QQC.Switch {
                id: writableSwitch
                Kirigami.FormData.label: root.t("kDrive folder:")
                text: root.t("Allow saving and editing files in it")
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: root.t("When off, the folder is read-only. When on, files you delete there go to the kDrive trash; moving and renaming are not possible.")
            }
            QQC.Switch {
                id: deletesSwitch
                Kirigami.FormData.label: root.t("Deleted files:")
                text: root.t("Also delete from the cloud what I delete on this computer")
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: root.t("The cloud copy goes to the kDrive trash and can be restored from there. If more than 20 files, or everything in a folder, are deleted at once, you are asked first.")
            }
            QQC.SpinBox {
                id: cacheSpin
                Kirigami.FormData.label: root.t("Cache limit (GB):")
                from: 1; to: 4000
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: root.t("Files you open from the cloud are kept on disk up to this limit; beyond it the least recently opened are removed.")
            }
            QQC.TextField {
                id: rootField
                Kirigami.FormData.label: root.t("Default backup place:")
                Layout.fillWidth: true
            }
            RowLayout {
                Kirigami.FormData.label: root.t("Not backed up:")
                spacing: Kirigami.Units.largeSpacing
                QQC.Button {
                    text: root.t("Edit list…")
                    icon.name: "document-edit"
                    onClicked: root.api("/api/ignore/open", {}, function() {})
                }
                QQC.Label {
                    opacity: 0.7
                    text: settingsDialog.cfg ? settingsDialog.cfg.exclude_count + root.t(" patterns") : ""
                }
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: root.t("Opens a text file like .gitignore: one file or folder name per line. Applied within seconds after you save.")
            }
            QQC.ComboBox {
                id: langBox
                Kirigami.FormData.label: root.t("Language:")
                textRole: "name"
            }
            QQC.SpinBox {
                id: rescanSpin
                Kirigami.FormData.label: root.t("Safety rescan (minutes):")
                from: 1; to: 1440
            }
            QQC.Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: settingsDialog.cfg ? root.t("Settings file: ") + root.shortPath(settingsDialog.cfg.config_path)
                                           + root.t(". kdlite restarts with the new settings when you save.") : ""
            }
          }
        }
    }

    // Ilk kurulum sihirbazi: ayar dosyasi yokken gosterilir.
    QQC.Page {
        id: wizard
        anchors.fill: parent
        visible: root.setup
        property var drives: []
        property var dirs: []
        property bool busy: false
        property bool done: false
        function post(path, body, ok) {
            busy = true
            root.errorText = ""
            var x = new XMLHttpRequest()
            x.onreadystatechange = function() {
                if (x.readyState !== XMLHttpRequest.DONE) return
                wizard.busy = false
                var j = null
                try { j = JSON.parse(x.responseText) } catch (e) {}
                if (x.status === 200) ok(j)
                else root.errorText = (j && j.error) ? j.error : root.t("Could not reach kdlite")
            }
            x.open("POST", root.base + path)
            x.setRequestHeader("Content-Type", "application/json")
            x.setRequestHeader("X-Kdlite", "1")
            x.send(JSON.stringify(body))
        }
        function connect() {
            post("/api/setup/drives", { token: tokenField.text }, function(j) {
                wizard.drives = j.drives
                driveBox.currentIndex = 0
                wizard.loadFolders()
            })
        }
        function loadFolders() {
            if (drives.length === 0) return
            post("/api/setup/folders", { token: tokenField.text, drive_id: drives[driveBox.currentIndex].id }, function(j) {
                wizard.dirs = [{ id: 1, name: root.t("Entire drive") }].concat(j.dirs)
                var pick = 0
                for (var i = 0; i < wizard.dirs.length; i++) if (wizard.dirs[i].name === "Private") pick = i
                rootBox.currentIndex = pick
            })
        }
        function finish() {
            post("/api/setup/save", { token: tokenField.text, drive_id: drives[driveBox.currentIndex].id,
                                      root_id: dirs[rootBox.currentIndex].id }, function() { wizard.done = true })
        }

        ColumnLayout {
            anchors.centerIn: parent
            width: Math.min(parent.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 30)
            spacing: Kirigami.Units.largeSpacing

            Kirigami.Icon {
                Layout.alignment: Qt.AlignHCenter
                source: "folder-cloud"
                implicitWidth: Kirigami.Units.iconSizes.huge
                implicitHeight: Kirigami.Units.iconSizes.huge
            }
            Kirigami.Heading {
                Layout.fillWidth: true
                horizontalAlignment: Text.AlignHCenter
                text: wizard.done ? root.t("Setup complete") : root.t("Connect kdlite to kDrive")
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                visible: wizard.done
                text: root.t("Starting kdlite…")
            }
            Kirigami.InlineMessage {
                Layout.fillWidth: true
                type: Kirigami.MessageType.Error
                text: root.errorText
                visible: root.setup && root.errorText.length > 0
            }

            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                visible: !wizard.done
                text: root.t("1. Create an API token in your Infomaniak account (scope: drive) and paste it here. The token is stored only in the settings file on this computer.")
            }
            RowLayout {
                Layout.fillWidth: true
                visible: !wizard.done
                QQC.TextField {
                    id: tokenField
                    Layout.fillWidth: true
                    echoMode: TextInput.Password
                    placeholderText: root.t("API token")
                    enabled: !wizard.busy
                    onTextEdited: { wizard.drives = []; wizard.dirs = [] }
                    onAccepted: if (text.length > 0) wizard.connect()
                }
                QQC.Button {
                    text: root.t("Connect")
                    icon.name: "network-connect"
                    enabled: !wizard.busy && tokenField.text.length > 0
                    onClicked: wizard.connect()
                }
            }
            QQC.Button {
                visible: !wizard.done
                flat: true
                text: root.t("Open the token page in the browser")
                icon.name: "internet-services"
                onClicked: Qt.openUrlExternally("https://manager.infomaniak.com/v3/ng/accounts/token/list")
            }

            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                visible: !wizard.done && wizard.drives.length > 0
                text: root.t("2. Choose the drive and the folder kdlite will see. Backups go inside this folder too.")
            }
            Kirigami.FormLayout {
                Layout.fillWidth: true
                visible: !wizard.done && wizard.drives.length > 0
                QQC.ComboBox {
                    id: driveBox
                    Kirigami.FormData.label: root.t("Drive:")
                    Layout.fillWidth: true
                    model: wizard.drives
                    textRole: "name"
                    onActivated: wizard.loadFolders()
                }
                QQC.ComboBox {
                    id: rootBox
                    Kirigami.FormData.label: root.t("Folder:")
                    Layout.fillWidth: true
                    model: wizard.dirs
                    textRole: "name"
                }
            }
            QQC.Button {
                Layout.alignment: Qt.AlignHCenter
                visible: !wizard.done && wizard.dirs.length > 0
                enabled: !wizard.busy
                text: root.t("Finish setup")
                icon.name: "dialog-ok-apply"
                highlighted: true
                onClicked: wizard.finish()
            }
            QQC.BusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                running: wizard.busy || wizard.done
                visible: running
            }
        }
    }

    QQC.Page {
        id: page
        anchors.fill: parent
        visible: !root.setup && root.view === "main"

        header: QQC.ToolBar {
            contentItem: RowLayout {
                spacing: Kirigami.Units.largeSpacing
                Kirigami.Icon {
                    source: "folder-cloud"
                    implicitWidth: Kirigami.Units.iconSizes.medium
                    implicitHeight: Kirigami.Units.iconSizes.medium
                    Kirigami.Icon {
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        anchors.margins: -2
                        width: Kirigami.Units.iconSizes.small
                        height: width
                        source: root.badgeIcon
                        visible: root.badgeIcon.length > 0
                    }
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 0
                    QQC.Label {
                        Layout.fillWidth: true
                        text: root.statusTitle
                        font.bold: true
                        elide: Text.ElideRight
                    }
                    QQC.Label {
                        Layout.fillWidth: true
                        text: root.statusDetail
                        visible: text.length > 0
                        opacity: 0.7
                        font: Kirigami.Theme.smallFont
                        elide: Text.ElideMiddle
                    }
                }
                QQC.Button {
                    text: root.paused ? root.t("Resume") : root.t("Pause")
                    icon.name: root.paused ? "media-playback-start" : "media-playback-pause"
                    enabled: root.st !== null && root.reachable
                    onClicked: root.api("/api/pause", { on: !root.paused }, function() { root.refresh() })
                    QQC.ToolTip.text: root.paused ? root.t("Continue backing up where it left off")
                                                  : root.t("Stop uploading; nothing is sent until you press “Resume”")
                    QQC.ToolTip.visible: hovered
                    QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                QQC.Button {
                    text: root.t("Back up now")
                    icon.name: "cloud-upload"
                    enabled: root.reachable && !root.running && root.sources.length > 0
                    onClicked: root.api("/api/run", {}, function() { root.refresh() })
                }
                QQC.ToolButton {
                    icon.name: "configure"
                    text: root.t("Settings")
                    display: QQC.AbstractButton.IconOnly
                    enabled: root.reachable && root.st !== null
                    QQC.ToolTip.text: text
                    QQC.ToolTip.visible: hovered
                    QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                    onClicked: settingsDialog.load()
                }
            }
        }

        ColumnLayout {
            anchors.fill: parent
            spacing: 0

            QQC.ProgressBar {
                Layout.fillWidth: true
                visible: root.running
                indeterminate: !root.running || root.st.progress.total_bytes === 0
                from: 0
                to: root.running ? Math.max(1, root.st.progress.total_bytes) : 1
                value: root.running ? root.st.progress.done_bytes : 0
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.smallSpacing
                type: Kirigami.MessageType.Error
                text: root.errorText
                visible: root.errorText.length > 0
                showCloseButton: true
                onVisibleChanged: if (!visible) root.errorText = ""
            }

            Kirigami.InlineMessage {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.smallSpacing
                visible: root.notice !== null
                type: root.notice && root.notice.errors.length > 0 ? Kirigami.MessageType.Warning
                                                                                           : Kirigami.MessageType.Positive
                text: {
                    var n = root.notice
                    if (!n) return ""
                    var t = n.mode === "cloud" ? n.done + root.t(" files are now in the cloud only (") + root.human(n.bytes) + ")."
                          : n.mode === "both" ? n.done + root.t(" files are now in the cloud and on this computer.")
                          : n.done + root.t(" files are now on this computer only; the cloud copies are in the kDrive trash.")
                    if (n.kept > 0) t += " " + n.kept + root.t(" files were left as they are.")
                    if (n.mode === "cloud" && n.done > 0) t += root.t(" Disk space is freed when you empty the trash.")
                    if (n.errors.length > 0) t += "\n" + n.errors.slice(0, 3).join("\n")
                    return t
                }
                showCloseButton: true
                onVisibleChanged: if (!visible && root.st && root.st.notice) root.noticeSeen = root.st.notice.at
            }

            RowLayout {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.largeSpacing
                Kirigami.Heading {
                    Layout.fillWidth: true
                    level: 4
                    text: root.t("Backed-up folders")
                }
                QQC.Button {
                    text: root.t("Add folder…")
                    icon.name: "folder-add"
                    enabled: root.reachable
                    onClicked: {
                        if (root.st) picker.currentFolder = "file://" + root.st.home
                        picker.open()
                    }
                }
            }

            QQC.ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                ListView {
                    id: list
                    clip: true
                    model: root.sources
                    spacing: 0

                    Kirigami.PlaceholderMessage {
                        anchors.centerIn: parent
                        width: parent.width - Kirigami.Units.gridUnit * 4
                        visible: list.count === 0 && root.st !== null
                        icon.name: "folder-cloud"
                        text: root.t("The list is empty")
                        explanation: root.t("Use “Add folder…” to choose the folders you want to back up.")
                    }

                    delegate: QQC.ItemDelegate {
                        id: row
                        required property var modelData
                        width: ListView.view.width
                        hoverEnabled: true
                        readonly property var stats: modelData.last ? modelData.last.stats : null
                        readonly property int pendingDeletes: stats && stats.pending_deletes ? stats.pending_deletes : 0
                        readonly property bool active: root.running && root.st.progress.source === modelData.name
                        readonly property string badge: active ? root.t("Backing up") : !modelData.exists ? root.t("Folder not found")
                            : root.paused ? root.t("Paused")
                            : (modelData.last && modelData.last.error) ? root.t("Error")
                            : (stats && stats.failed > 0) ? stats.failed + root.t(" files failed")
                            : stats ? root.t("Backed up") : root.t("Waiting")
                        readonly property string emblem: active ? "state-sync" : !modelData.exists ? "emblem-warning"
                            : root.paused ? "emblem-pause" : bad ? "emblem-warning" : stats ? "emblem-checked" : ""
                        readonly property bool bad: !modelData.exists || (modelData.last && modelData.last.error) || (stats && stats.failed > 0)
                        readonly property var errors: (modelData.last && modelData.last.error) ? [modelData.last.error]
                            : (stats && stats.errors) ? stats.errors.slice(0, 3) : []

                        contentItem: RowLayout {
                            spacing: Kirigami.Units.largeSpacing
                            Kirigami.Icon {
                                source: "folder-cloud"
                                implicitWidth: Kirigami.Units.iconSizes.medium
                                implicitHeight: Kirigami.Units.iconSizes.medium
                                Kirigami.Icon {
                                    anchors.right: parent.right
                                    anchors.bottom: parent.bottom
                                    anchors.margins: -2
                                    width: Kirigami.Units.iconSizes.small
                                    height: width
                                    source: row.emblem
                                    visible: row.emblem.length > 0
                                }
                            }
                            ColumnLayout {
                                Layout.fillWidth: true
                                spacing: 2
                                RowLayout {
                                    spacing: Kirigami.Units.smallSpacing
                                    QQC.Label { text: row.modelData.name; font.bold: true; elide: Text.ElideRight; Layout.maximumWidth: row.width * 0.5 }
                                    QQC.Label {
                                        text: row.badge
                                        font: Kirigami.Theme.smallFont
                                        color: row.bad ? Kirigami.Theme.negativeTextColor
                                             : (root.paused && !row.active) ? Kirigami.Theme.neutralTextColor
                                             : row.stats ? Kirigami.Theme.positiveTextColor : Kirigami.Theme.disabledTextColor
                                    }
                                }
                                QQC.Label {
                                    Layout.fillWidth: true
                                    text: root.shortPath(row.modelData.local) + "  →  " + row.modelData.remote
                                    opacity: 0.7
                                    font: Kirigami.Theme.smallFont
                                    elide: Text.ElideMiddle
                                }
                                QQC.Label {
                                    Layout.fillWidth: true
                                    text: row.modelData.files + root.t(" files · ") + root.human(row.modelData.bytes) + root.t(" in the cloud")
                                          + (row.modelData.cloud_only > 0 ? " · " + row.modelData.cloud_only + root.t(" cloud only") : "")
                                          + (row.modelData.local_only > 0 ? " · " + row.modelData.local_only + root.t(" this computer only") : "")
                                          + (row.modelData.last ? root.t("   ·   last check ") + root.ago(row.modelData.last.at) : "")
                                    opacity: 0.7
                                    font: Kirigami.Theme.smallFont
                                    elide: Text.ElideRight
                                }
                                // Toplu silme onayi.
                                RowLayout {
                                    Layout.fillWidth: true
                                    visible: row.pendingDeletes > 0
                                    spacing: Kirigami.Units.smallSpacing
                                    QQC.Label {
                                        Layout.fillWidth: true
                                        wrapMode: Text.WordWrap
                                        font: Kirigami.Theme.smallFont
                                        color: Kirigami.Theme.neutralTextColor
                                        text: row.pendingDeletes + root.t(" files were deleted on this computer. Delete them from the cloud too?")
                                    }
                                    QQC.Button {
                                        text: root.t("Delete from cloud too")
                                        icon.name: "edit-delete"
                                        onClicked: root.api("/api/deletes", { name: row.modelData.name, apply: true }, function() { root.refresh() })
                                        QQC.ToolTip.text: root.t("Moved to the kDrive trash; can be restored from there")
                                        QQC.ToolTip.visible: hovered
                                        QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                                    }
                                    QQC.Button {
                                        text: root.t("Keep in cloud")
                                        onClicked: root.api("/api/deletes", { name: row.modelData.name, apply: false }, function() { root.refresh() })
                                    }
                                }
                                Repeater {
                                    model: row.errors
                                    QQC.Label {
                                        required property string modelData
                                        Layout.fillWidth: true
                                        text: modelData
                                        color: Kirigami.Theme.negativeTextColor
                                        font: Kirigami.Theme.smallFont
                                        wrapMode: Text.WrapAnywhere
                                        maximumLineCount: 2
                                        elide: Text.ElideRight
                                    }
                                }
                            }
                            QQC.ToolButton {
                                icon.name: "configure"
                                text: root.t("Change its place in the cloud")
                                display: QQC.AbstractButton.IconOnly
                                enabled: !root.running
                                QQC.ToolTip.text: text
                                QQC.ToolTip.visible: hovered
                                QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                                onClicked: {
                                    destDialog.editName = row.modelData.name
                                    destDialog.localPath = row.modelData.local
                                    destDialog.stack = []
                                    destDialog.load()
                                    destDialog.open()
                                }
                            }
                            QQC.ToolButton {
                                id: placeButton
                                icon.name: "cloudstatus"
                                text: root.t("Where should the files be kept?")
                                display: QQC.AbstractButton.IconOnly
                                enabled: !root.running && row.modelData.exists && row.stats !== null
                                QQC.ToolTip.text: text
                                QQC.ToolTip.visible: hovered && !placeMenu.visible
                                QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                                onClicked: placeMenu.popup(placeButton, 0, placeButton.height)
                                QQC.Menu {
                                    id: placeMenu
                                    QQC.MenuItem {
                                        text: root.t("Keep in cloud only")
                                        icon.name: "cloudstatus"
                                        onTriggered: {
                                            root.pendingFree = row.modelData.name
                                            root.pendingFreePath = row.modelData.local
                                            confirmFree.open()
                                        }
                                    }
                                    QQC.MenuItem {
                                        text: root.t("Keep in cloud and on this computer")
                                        icon.name: "folder-cloud"
                                        onTriggered: root.place(row.modelData.local, "both")
                                    }
                                }
                            }
                            QQC.ToolButton {
                                icon.name: "list-remove"
                                text: root.t("Remove from list")
                                display: QQC.AbstractButton.IconOnly
                                QQC.ToolTip.text: text
                                QQC.ToolTip.visible: hovered
                                QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                                onClicked: { root.pendingRemove = row.modelData.name; confirmRemove.open() }
                            }
                        }
                    }
                }
            }
        }

        footer: QQC.ToolBar {
            contentItem: QQC.Label {
                text: root.st ? root.t("Older versions of changed files stay in kDrive version history. Right-click a file or folder and use the kdlite menu to choose where it is kept.")
                              : ""
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
            }
        }
    }

    // Gelistirme icin: "-- --shot=/yol/dosya.png" verilirse pencereyi resme kaydedip cikar.
    Timer {
        property string target: {
            var a = Qt.application.arguments
            for (var i = 0; i < a.length; i++) if (a[i].indexOf("--shot=") === 0) return a[i].slice(7)
            return ""
        }
        property string demoToken: {
            var a = Qt.application.arguments
            for (var i = 0; i < a.length; i++) if (a[i].indexOf("--demo-token=") === 0) return a[i].slice(13)
            return ""
        }
        property Timer demo: Timer {
            interval: 800
            running: parent.demoToken.length > 0
            onTriggered: { tokenField.text = parent.demoToken; wizard.connect() }
        }
        property Timer demoSettings: Timer {
            interval: 800
            running: Qt.application.arguments.indexOf("--demo-settings") >= 0
            onTriggered: settingsDialog.load()
        }
        property Timer demoDest: Timer {
            interval: 800
            running: Qt.application.arguments.indexOf("--demo-dest") >= 0
            onTriggered: { destDialog.localPath = "/home/u/Belgelerim"; destDialog.stack = []; destDialog.load(); destDialog.open() }
        }
        interval: 2500
        running: target.length > 0
        onTriggered: (destDialog.visible ? root.contentItem : settingsDialog.visible ? settingsDialog : root.setup ? wizard : page).grabToImage(function(r) { r.saveToFile(target); Qt.quit() })
    }
}
