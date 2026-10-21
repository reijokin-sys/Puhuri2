// SPDX-License-Identifier: AGPL-3.0-or-later
// Report Qt's advertised recorder formats without opening the microphone.
#include <QCoreApplication>
#include <QAudioRecorder>
#include <QDebug>

int main(int argc, char **argv)
{
    QCoreApplication application(argc, argv);
    QAudioRecorder recorder;
    qInfo() << "Audio codecs:" << recorder.supportedAudioCodecs();
    qInfo() << "Containers:" << recorder.supportedContainers();
    return recorder.supportedAudioCodecs().contains(QStringLiteral("audio/x-opus"))
            && recorder.supportedContainers().contains(QStringLiteral("audio/ogg")) ? 0 : 1;
}
