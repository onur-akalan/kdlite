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
        if (s < 60) return "az önce"
        if (s < 3600) return Math.floor(s / 60) + " dk önce"
        if (s < 86400) return Math.floor(s / 3600) + " saat önce"
        return Math.floor(s / 86400) + " gün önce"
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
                root.errorText = (j && j.error) ? j.error : "İstek başarısız"
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
        if (!reachable) return "kdlite çalışmıyor"
        if (!st) return "Yükleniyor…"
        if (running && st.freeing) return "Dosyaların konumu değiştiriliyor"
        if (running) {
            var p = st.progress
            return p.total_files > 0 ? "Yedekleniyor: " + p.source + " · " + p.done_files + "/" + p.total_files + " dosya"
                                     : "Taranıyor: " + (p.source || "…")
        }
        if (paused) return "Yedekleme duraklatıldı"
        if (sources.length === 0) return "Henüz klasör seçilmedi"
        if (anyFailed) return "Bazı dosyalar yedeklenemedi"
        for (var i = 0; i < sources.length; i++) if (!sources[i].last) return "Yedekleme bekliyor"
        return "Her şey yedekli"
    }
    readonly property string statusDetail: {
        if (!reachable) return "Tepsi simgesinden kdlite'ı yeniden başlat."
        if (!st) return ""
        if (running && st.freeing) return st.progress.done_files + "/" + st.progress.total_files + " dosya"
        if (running) {
            var p = st.progress
            return p.total_files > 0 ? human(p.done_bytes) + " / " + human(p.total_bytes) + " · " + p.current : ""
        }
        if (paused) return "“Sürdür”e basana kadar hiçbir şey yüklenmez."
        if (sources.length === 0) return "Yedeklemek istediğin klasörleri ekle; değişiklikler kendiliğinden kDrive'a akar."
        if (st.mount_failed > 0) return "kDrive klasörüne kaydedilen " + st.mount_failed + " dosya yüklenemedi; yeniden denenecek."
        if (st.mount_pending > 0) return "kDrive klasörüne kaydedilen " + st.mount_pending + " dosya yükleniyor…"
        return st.watching ? "Klasörler izleniyor; değişiklikler birkaç saniye içinde yüklenir."
                           : "Değişiklikler " + Math.max(1, Math.round(st.interval / 60)) + " dakikada bir kontrol edilip yükleniyor."
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
        title: "Yedeklenecek klasörü seç"
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
        title: "kDrive'da nereye yedeklensin?"
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
                text: "“" + destDialog.folderName + "” için buluttaki yeri seç. Klasör, seçtiğin yerin içinde kendi adıyla oluşturulur."
                      + (destDialog.editName ? " Eski yerdeki kopya silinmez; dosyalar yeni yere yeniden yüklenir." : "")
            }
            RowLayout {
                Layout.fillWidth: true
                QQC.ToolButton {
                    icon.name: "go-up"
                    text: "Yukarı"
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
                        text: destDialog.loading ? "Yükleniyor…" : "Alt klasör yok"
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
                text: "Hedef: " + (destDialog.here ? destDialog.here + "/" : "") + destDialog.folderName
            }
        }
        footer: QQC.DialogButtonBox {
            QQC.Button {
                text: "Varsayılan yer (" + (root.st ? root.st.backup_root : "") + ")"
                QQC.DialogButtonBox.buttonRole: QQC.DialogButtonBox.ActionRole
                onClicked: destDialog.add(undefined)
            }
            QQC.Button {
                text: "Buraya yedekle"
                icon.name: "dialog-ok-apply"
                QQC.DialogButtonBox.buttonRole: QQC.DialogButtonBox.AcceptRole
                onClicked: destDialog.add(destDialog.here)
            }
            QQC.Button {
                text: "Vazgeç"
                QQC.DialogButtonBox.buttonRole: QQC.DialogButtonBox.RejectRole
                onClicked: destDialog.close()
            }
        }
    }

    QQC.Dialog {
        id: confirmRemove
        title: "Listeden çıkar"
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 24)
        standardButtons: QQC.Dialog.Ok | QQC.Dialog.Cancel
        QQC.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "“" + root.pendingRemove + "” artık yedeklenmeyecek. Buluttaki kopya silinmez."
        }
        onAccepted: root.api("/api/remove", { name: root.pendingRemove }, function() { root.refresh() })
    }

    QQC.Dialog {
        id: confirmLogout
        title: "Hesaptan çık"
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
        standardButtons: QQC.Dialog.Ok | QQC.Dialog.Cancel
        QQC.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "API token'ı bu bilgisayardan silinir ve kDrive klasörünün bağlantısı kesilir. "
                + "Yedek listen, ayarların ve buluttaki dosyaların yerinde kalır.\n\n"
                + "Ardından kurulum sihirbazı açılır; aynı ya da başka bir token'la yeniden bağlanabilirsin."
        }
        onAccepted: { root.view = "main"; root.api("/api/logout", {}, function() {}) }
    }

    QQC.Dialog {
        id: confirmFree
        title: "Yalnızca bulutta tut"
        modal: true
        parent: QQC.Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(root.width - Kirigami.Units.gridUnit * 4, Kirigami.Units.gridUnit * 28)
        standardButtons: QQC.Dialog.Ok | QQC.Dialog.Cancel
        QQC.Label {
            width: parent.width
            wrapMode: Text.WordWrap
            text: "“" + root.pendingFree + "” içindeki dosyalar bilgisayardan kaldırılır ve yalnızca bulutta kalır. "
                + "Yerlerinde birer kısayol durur: dosyayı açtığında buluttan iner.\n\n"
                + "Yalnızca bulutta birebir aynısı durduğu şimdi doğrulanan dosyalar kaldırılır; diğerleri yerinde kalır. "
                + "Kaldırılanlar çöp kutusuna gider; disk alanı çöp kutusunu boşaltınca açılır."
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
                    text: "Geri"
                    onClicked: root.view = "main"
                }
                Kirigami.Heading {
                    Layout.fillWidth: true
                    level: 3
                    text: "Ayarlar"
                }
                QQC.Button {
                    text: "Kaydet"
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
                Kirigami.FormData.label: "Hesap:"
                spacing: Kirigami.Units.largeSpacing
                Kirigami.Icon { source: "folder-cloud"; implicitWidth: Kirigami.Units.iconSizes.small; implicitHeight: implicitWidth }
                QQC.Label { text: settingsDialog.cfg ? settingsDialog.cfg.drive : "" }
                QQC.Button {
                    text: "Hesaptan çık…"
                    icon.name: "system-log-out"
                    onClicked: confirmLogout.open()
                }
            }
            QQC.Switch {
                id: autostartSwitch
                Kirigami.FormData.label: "Başlangıç:"
                text: "Oturum açılınca kendiliğinden başlat"
            }
            QQC.Switch {
                id: writableSwitch
                Kirigami.FormData.label: "kDrive klasörü:"
                text: "İçine dosya kaydetmeye ve düzenlemeye izin ver"
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: "Kapalıyken klasör salt okunurdur. Açıkken orada sildiğin dosya kDrive çöp kutusuna gider; taşıma ve yeniden adlandırma yapılamaz."
            }
            QQC.Switch {
                id: deletesSwitch
                Kirigami.FormData.label: "Silinen dosyalar:"
                text: "Bilgisayarda sildiğim dosya buluttan da silinsin"
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: "Buluttaki kopya kDrive çöp kutusuna gider, oradan geri alınabilir. Bir seferde 20'den fazla dosya ya da klasörün tamamı silinirse önce onay istenir."
            }
            QQC.SpinBox {
                id: cacheSpin
                Kirigami.FormData.label: "Önbellek sınırı (GB):"
                from: 1; to: 4000
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: "Buluttan açtığın dosyalar bu sınıra kadar diskte tutulur; aşılınca en eski açılanlar silinir."
            }
            QQC.TextField {
                id: rootField
                Kirigami.FormData.label: "Varsayılan yedek yeri:"
                Layout.fillWidth: true
            }
            RowLayout {
                Kirigami.FormData.label: "Yedeklenmeyecekler:"
                spacing: Kirigami.Units.largeSpacing
                QQC.Button {
                    text: "Listeyi düzenle…"
                    icon.name: "document-edit"
                    onClicked: root.api("/api/ignore/open", {}, function() {})
                }
                QQC.Label {
                    opacity: 0.7
                    text: settingsDialog.cfg ? settingsDialog.cfg.exclude_count + " desen" : ""
                }
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: ".gitignore gibi bir metin dosyası açılır: her satıra bir dosya ya da klasör adı. Kaydettiğinde birkaç saniye içinde uygulanır."
            }
            QQC.SpinBox {
                id: rescanSpin
                Kirigami.FormData.label: "Güvenlik taraması (dakika):"
                from: 1; to: 1440
            }
            QQC.Label {
                Layout.fillWidth: true
                elide: Text.ElideMiddle
                opacity: 0.7
                font: Kirigami.Theme.smallFont
                text: settingsDialog.cfg ? "Ayar dosyası: " + root.shortPath(settingsDialog.cfg.config_path)
                                           + ". Kaydedince kdlite yeni ayarlarla yeniden başlar." : ""
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
                else root.errorText = (j && j.error) ? j.error : "kdlite'a ulaşılamadı"
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
                wizard.dirs = [{ id: 1, name: "Sürücünün tamamı" }].concat(j.dirs)
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
                text: wizard.done ? "Kurulum tamam" : "kdlite'ı kDrive'a bağla"
            }
            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
                visible: wizard.done
                text: "kdlite başlatılıyor…"
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
                text: "1. Infomaniak hesabında bir API token'ı oluştur (yetki: drive) ve buraya yapıştır. Token yalnızca bu bilgisayardaki ayar dosyasına kaydedilir."
            }
            RowLayout {
                Layout.fillWidth: true
                visible: !wizard.done
                QQC.TextField {
                    id: tokenField
                    Layout.fillWidth: true
                    echoMode: TextInput.Password
                    placeholderText: "API token'ı"
                    enabled: !wizard.busy
                    onTextEdited: { wizard.drives = []; wizard.dirs = [] }
                    onAccepted: if (text.length > 0) wizard.connect()
                }
                QQC.Button {
                    text: "Bağlan"
                    icon.name: "network-connect"
                    enabled: !wizard.busy && tokenField.text.length > 0
                    onClicked: wizard.connect()
                }
            }
            QQC.Button {
                visible: !wizard.done
                flat: true
                text: "Token sayfasını tarayıcıda aç"
                icon.name: "internet-services"
                onClicked: Qt.openUrlExternally("https://manager.infomaniak.com/v3/ng/accounts/token/list")
            }

            QQC.Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                visible: !wizard.done && wizard.drives.length > 0
                text: "2. Sürücüyü ve kdlite'ın göreceği klasörü seç. Yedekler de bu klasörün içine gider."
            }
            Kirigami.FormLayout {
                Layout.fillWidth: true
                visible: !wizard.done && wizard.drives.length > 0
                QQC.ComboBox {
                    id: driveBox
                    Kirigami.FormData.label: "Sürücü:"
                    Layout.fillWidth: true
                    model: wizard.drives
                    textRole: "name"
                    onActivated: wizard.loadFolders()
                }
                QQC.ComboBox {
                    id: rootBox
                    Kirigami.FormData.label: "Klasör:"
                    Layout.fillWidth: true
                    model: wizard.dirs
                    textRole: "name"
                }
            }
            QQC.Button {
                Layout.alignment: Qt.AlignHCenter
                visible: !wizard.done && wizard.dirs.length > 0
                enabled: !wizard.busy
                text: "Kurulumu bitir"
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
                    text: root.paused ? "Sürdür" : "Duraklat"
                    icon.name: root.paused ? "media-playback-start" : "media-playback-pause"
                    enabled: root.st !== null && root.reachable
                    onClicked: root.api("/api/pause", { on: !root.paused }, function() { root.refresh() })
                    QQC.ToolTip.text: root.paused ? "Yedeklemeye kaldığı yerden devam et"
                                                  : "Yüklemeyi durdur; “Sürdür”e basana kadar hiçbir şey gönderilmez"
                    QQC.ToolTip.visible: hovered
                    QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                QQC.Button {
                    text: "Şimdi yedekle"
                    icon.name: "cloud-upload"
                    enabled: root.reachable && !root.running && root.sources.length > 0
                    onClicked: root.api("/api/run", {}, function() { root.refresh() })
                }
                QQC.ToolButton {
                    icon.name: "configure"
                    text: "Ayarlar"
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
                    var t = n.mode === "cloud" ? n.done + " dosya artık yalnızca bulutta (" + root.human(n.bytes) + ")."
                          : n.mode === "both" ? n.done + " dosya artık bulutta ve bilgisayarda."
                          : n.done + " dosya artık yalnızca bilgisayarda; buluttaki kopyalar kDrive çöp kutusunda."
                    if (n.kept > 0) t += " " + n.kept + " dosya olduğu gibi bırakıldı."
                    if (n.mode === "cloud" && n.done > 0) t += " Disk alanı, çöp kutusunu boşaltınca açılır."
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
                    text: "Yedeklenen klasörler"
                }
                QQC.Button {
                    text: "Klasör ekle…"
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
                        text: "Liste boş"
                        explanation: "“Klasör ekle…” ile yedeklemek istediğin klasörleri seç."
                    }

                    delegate: QQC.ItemDelegate {
                        id: row
                        required property var modelData
                        width: ListView.view.width
                        hoverEnabled: true
                        readonly property var stats: modelData.last ? modelData.last.stats : null
                        readonly property int pendingDeletes: stats && stats.pending_deletes ? stats.pending_deletes : 0
                        readonly property bool active: root.running && root.st.progress.source === modelData.name
                        readonly property string badge: active ? "Yedekleniyor" : !modelData.exists ? "Klasör bulunamadı"
                            : root.paused ? "Duraklatıldı"
                            : (modelData.last && modelData.last.error) ? "Hata"
                            : (stats && stats.failed > 0) ? stats.failed + " dosya başarısız"
                            : stats ? "Yedekli" : "Bekliyor"
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
                                    text: row.modelData.files + " dosya · " + root.human(row.modelData.bytes) + " bulutta"
                                          + (row.modelData.cloud_only > 0 ? " · " + row.modelData.cloud_only + " yalnızca bulutta" : "")
                                          + (row.modelData.local_only > 0 ? " · " + row.modelData.local_only + " yalnızca bilgisayarda" : "")
                                          + (row.modelData.last ? "   ·   son kontrol " + root.ago(row.modelData.last.at) : "")
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
                                        text: row.pendingDeletes + " dosya bilgisayardan silinmiş. Buluttan da silinsin mi?"
                                    }
                                    QQC.Button {
                                        text: "Buluttan da sil"
                                        icon.name: "edit-delete"
                                        onClicked: root.api("/api/deletes", { name: row.modelData.name, apply: true }, function() { root.refresh() })
                                        QQC.ToolTip.text: "kDrive çöp kutusuna taşınır; oradan geri alınabilir"
                                        QQC.ToolTip.visible: hovered
                                        QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                                    }
                                    QQC.Button {
                                        text: "Bulutta kalsın"
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
                                text: "Buluttaki yeri değiştir"
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
                                text: "Dosyalar nerede dursun?"
                                display: QQC.AbstractButton.IconOnly
                                enabled: !root.running && row.modelData.exists && row.stats !== null
                                QQC.ToolTip.text: text
                                QQC.ToolTip.visible: hovered && !placeMenu.visible
                                QQC.ToolTip.delay: Kirigami.Units.toolTipDelay
                                onClicked: placeMenu.popup(placeButton, 0, placeButton.height)
                                QQC.Menu {
                                    id: placeMenu
                                    QQC.MenuItem {
                                        text: "Yalnızca bulutta tut"
                                        icon.name: "cloudstatus"
                                        onTriggered: {
                                            root.pendingFree = row.modelData.name
                                            root.pendingFreePath = row.modelData.local
                                            confirmFree.open()
                                        }
                                    }
                                    QQC.MenuItem {
                                        text: "Bulutta ve bilgisayarda tut"
                                        icon.name: "folder-cloud"
                                        onTriggered: root.place(row.modelData.local, "both")
                                    }
                                }
                            }
                            QQC.ToolButton {
                                icon.name: "list-remove"
                                text: "Listeden çıkar"
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
                text: root.st ? "Değişen dosyanın eski hali kDrive sürüm geçmişinde durur. Dosya ya da klasöre sağ tıklayıp kdlite menüsünden nerede duracağını seçebilirsin."
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
