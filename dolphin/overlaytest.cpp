// Gelistirme icin: eklentiyi Dolphin'in yukledigi gibi yukler ve verilen
// yollarin rozetlerini yazdirir. Kullanim: overlaytest <eklenti.so> <yol>...
#include <KOverlayIconPlugin>
#include <QCoreApplication>
#include <QPluginLoader>
#include <QTextStream>
#include <QTimer>
#include <QUrl>

int main(int argc, char **argv)
{
    QCoreApplication app(argc, argv);
    QTextStream out(stdout);
    QPluginLoader loader(QString::fromLocal8Bit(argv[1]));
    auto *plugin = qobject_cast<KOverlayIconPlugin *>(loader.instance());
    if (!plugin) {
        out << "yuklenemedi: " << loader.errorString() << "\n";
        return 1;
    }
    QObject::connect(plugin, &KOverlayIconPlugin::overlaysChanged, [&](const QUrl &u, const QStringList &o) {
        out << "DEGISTI " << u.toLocalFile() << " -> " << o.join(',') << "\n";
        out.flush();
    });
    for (int i = 2; i < argc; ++i) {
        const QString p = QString::fromLocal8Bit(argv[i]);
        if (p.startsWith("--wait=")) {
            continue;
        }
        out << p << " -> " << plugin->getOverlays(QUrl::fromLocalFile(p)).join(',') << "\n";
    }
    out.flush();
    int wait = 0;
    for (int i = 2; i < argc; ++i) {
        const QString p = QString::fromLocal8Bit(argv[i]);
        if (p.startsWith("--wait=")) {
            wait = p.mid(7).toInt();
        }
    }
    if (wait > 0) {
        QTimer::singleShot(wait * 1000, &app, &QCoreApplication::quit);
        return app.exec();
    }
    return 0;
}
