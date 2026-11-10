// Dolphin icin durum rozetleri: kdlite ile yedeklenen klasorlerdeki dosyalarin
// simgesine kucuk bir isaret ekler (yedekli / bekliyor / duraklatildi / hata).
//
// Eklenti kdlite servisiyle konusmaz; yalnizca servisin ~/.config/kdlite
// altina yazdigi durum dosyalarini okur (token'in durdugu config.toml haric).
#include <KOverlayIconPlugin>
#include <QDateTime>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QFileSystemWatcher>
#include <QHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSet>
#include <QStandardPaths>
#include <QTimer>
#include <QUrl>

namespace {
const QString OK = QStringLiteral("vcs-normal");
const QString PENDING = QStringLiteral("state-sync");
const QString PAUSED = QStringLiteral("emblem-pause");
const QString WARN = QStringLiteral("emblem-warning");
const QString CLOUD_ONLY = QStringLiteral("cloudstatus");
const QString LOCAL_ONLY = QStringLiteral("computer");
const int MAX_TRACKED = 20000;

struct Rec {
    qint64 size;
    qint64 mtime;
};
struct Source {
    QString root; // sonunda '/' ile
    QString name;
    bool failed = false;
    QHash<QString, Rec> files;
    QSet<QString> cloudOnly; // yerelde yalnizca kisayolu olanlar
    QSet<QString> localOnly; // yedeklenmeyenler
};

QJsonDocument readJson(const QString &path)
{
    QFile f(path);
    if (!f.open(QIODevice::ReadOnly)) {
        return {};
    }
    return QJsonDocument::fromJson(f.readAll());
}

// kdlite ile ayni basit desen: "*.ext", "onek*" ya da tam ad.
bool matches(const QString &pattern, const QString &name)
{
    if (pattern.startsWith(QLatin1Char('*'))) {
        return name.endsWith(pattern.mid(1));
    }
    if (pattern.endsWith(QLatin1Char('*'))) {
        return name.startsWith(pattern.left(pattern.size() - 1));
    }
    return pattern == name;
}
}

class KdliteOverlay : public KOverlayIconPlugin
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID "org.kdlite.overlayiconplugin" FILE "kdliteoverlay.json")

public:
    explicit KdliteOverlay(QObject *parent = nullptr)
        : KOverlayIconPlugin(parent)
    {
        const QString base = QStandardPaths::writableLocation(QStandardPaths::GenericConfigLocation);
        m_dir = base + QStringLiteral("/kdlite");
        m_reload.setSingleShot(true);
        m_reload.setInterval(700);
        connect(&m_reload, &QTimer::timeout, this, &KdliteOverlay::reload);
        // Dosyalar atomik olarak (yeniden adlandirmayla) yazildigi icin klasor izlenir.
        if (QDir(m_dir).exists()) {
            m_watcher.addPath(m_dir);
        }
        connect(&m_watcher, &QFileSystemWatcher::directoryChanged, &m_reload, qOverload<>(&QTimer::start));
        load();
    }

    QStringList getOverlays(const QUrl &url) override
    {
        if (!url.isLocalFile() || m_sources.isEmpty()) {
            return {};
        }
        const QString path = url.toLocalFile();
        const QStringList now = compute(path);
        if (m_shown.size() < MAX_TRACKED || m_shown.contains(path)) {
            if (now.isEmpty() && !inAnySource(path)) {
                return now; // ilgisiz yollari hatirlama
            }
            m_shown.insert(path, now);
        }
        return now;
    }

private:
    bool inAnySource(const QString &path) const
    {
        for (const Source &s : m_sources) {
            if (path.startsWith(s.root) || path + QLatin1Char('/') == s.root) {
                return true;
            }
        }
        return false;
    }

    QStringList compute(const QString &path) const
    {
        for (const Source &s : m_sources) {
            if (path + QLatin1Char('/') == s.root) {
                // Yedeklenen klasorun kendisi.
                return {m_paused ? PAUSED : s.failed ? WARN : OK};
            }
            if (!path.startsWith(s.root)) {
                continue;
            }
            const QString rel = path.mid(s.root.size());
            const QStringList parts = rel.split(QLatin1Char('/'));
            for (const QString &part : parts) {
                for (const QString &pat : m_exclude) {
                    if (matches(pat, part)) {
                        return {}; // yedeklemeden haric
                    }
                }
            }
            const QFileInfo fi(path);
            if (fi.isSymLink()) {
                return s.cloudOnly.contains(rel) ? QStringList{CLOUD_ONLY} : QStringList{};
            }
            if (!fi.isFile()) {
                return {}; // alt klasorler: rozet yok
            }
            if (s.localOnly.contains(rel)) {
                return {LOCAL_ONLY};
            }
            const auto it = s.files.constFind(rel);
            if (it != s.files.constEnd() && it->size == fi.size() && it->mtime == fi.lastModified().toSecsSinceEpoch()) {
                return {OK};
            }
            return {m_paused ? PAUSED : PENDING};
        }
        return {};
    }

    void load()
    {
        m_paused = QFile::exists(m_dir + QStringLiteral("/paused"));

        m_exclude.clear();
        const QJsonArray ex = readJson(m_dir + QStringLiteral("/exclude.json")).array();
        for (const auto &v : ex) {
            m_exclude << v.toString();
        }

        const QJsonObject last = readJson(m_dir + QStringLiteral("/backup-last.json")).object();
        const QJsonObject roots = readJson(m_dir + QStringLiteral("/backup-state.json")).object().value(QLatin1String("roots")).toObject();

        m_sources.clear();
        const QJsonArray list = readJson(m_dir + QStringLiteral("/backups.json")).array();
        for (const auto &v : list) {
            const QJsonObject o = v.toObject();
            Source s;
            s.name = o.value(QLatin1String("name")).toString();
            s.root = o.value(QLatin1String("local")).toString();
            if (s.root.isEmpty()) {
                continue;
            }
            if (!s.root.endsWith(QLatin1Char('/'))) {
                s.root += QLatin1Char('/');
            }
            const QJsonObject l = last.value(s.name).toObject();
            s.failed = !l.value(QLatin1String("error")).isNull() && !l.value(QLatin1String("error")).isUndefined();
            s.failed = s.failed || l.value(QLatin1String("stats")).toObject().value(QLatin1String("failed")).toInteger() > 0;
            const QJsonObject root = roots.value(s.name).toObject();
            for (const auto &x : root.value(QLatin1String("cloud_only")).toArray()) {
                s.cloudOnly.insert(x.toString());
            }
            for (const auto &x : root.value(QLatin1String("local_only")).toArray()) {
                s.localOnly.insert(x.toString());
            }
            const QJsonObject files = root.value(QLatin1String("files")).toObject();
            s.files.reserve(files.size());
            for (auto it = files.constBegin(); it != files.constEnd(); ++it) {
                const QJsonObject r = it.value().toObject();
                s.files.insert(it.key(), Rec{r.value(QLatin1String("size")).toInteger(), r.value(QLatin1String("mtime")).toInteger()});
            }
            m_sources.push_back(std::move(s));
        }
    }

    // Durum dosyalari degisti: gosterilen dosyalarin rozetini yeniden hesapla,
    // degisenleri Dolphin'e bildir.
    void reload()
    {
        if (m_watcher.directories().isEmpty() && QDir(m_dir).exists()) {
            m_watcher.addPath(m_dir);
        }
        load();
        for (auto it = m_shown.begin(); it != m_shown.end();) {
            const QStringList now = compute(it.key());
            if (now != it.value()) {
                Q_EMIT overlaysChanged(QUrl::fromLocalFile(it.key()), now);
                it.value() = now;
            }
            if (now.isEmpty() && !inAnySource(it.key())) {
                it = m_shown.erase(it);
            } else {
                ++it;
            }
        }
    }

    QString m_dir;
    QList<Source> m_sources;
    QStringList m_exclude;
    bool m_paused = false;
    QHash<QString, QStringList> m_shown;
    QFileSystemWatcher m_watcher;
    QTimer m_reload;
};

#include "kdliteoverlay.moc"
