// SPDX-License-Identifier: AGPL-3.0-or-later
#include <QAudioEncoderSettings>
#include <QAudioRecorder>
#include <QQmlEngine>
#include <QQmlExtensionPlugin>
#include <QUrl>

class PuhuriAudioRecorder : public QObject
{
    Q_OBJECT
    Q_PROPERTY(State recorderState READ recorderState NOTIFY recorderStateChanged)
    Q_PROPERTY(Status recorderStatus READ recorderStatus NOTIFY recorderStatusChanged)
    Q_PROPERTY(Error errorCode READ errorCode NOTIFY errorChanged)
    Q_PROPERTY(QString errorString READ errorString NOTIFY errorChanged)
    Q_PROPERTY(QString outputLocation READ outputLocation WRITE setOutputLocation NOTIFY outputLocationChanged)
    Q_PROPERTY(QString actualLocation READ actualLocation NOTIFY actualLocationChanged)
    Q_PROPERTY(qint64 duration READ duration NOTIFY durationChanged)
    Q_PROPERTY(QStringList supportedAudioCodecs READ supportedAudioCodecs CONSTANT)
    Q_PROPERTY(QStringList supportedContainers READ supportedContainers CONSTANT)

public:
    enum State {
        StoppedState = QMediaRecorder::StoppedState,
        RecordingState = QMediaRecorder::RecordingState,
        PausedState = QMediaRecorder::PausedState
    };
    Q_ENUM(State)

    enum Status {
        UnavailableStatus = QMediaRecorder::UnavailableStatus,
        UnloadedStatus = QMediaRecorder::UnloadedStatus,
        LoadingStatus = QMediaRecorder::LoadingStatus,
        LoadedStatus = QMediaRecorder::LoadedStatus,
        StartingStatus = QMediaRecorder::StartingStatus,
        RecordingStatus = QMediaRecorder::RecordingStatus,
        PausedStatus = QMediaRecorder::PausedStatus,
        FinalizingStatus = QMediaRecorder::FinalizingStatus
    };
    Q_ENUM(Status)

    enum Error {
        NoError = QMediaRecorder::NoError,
        ResourceError = QMediaRecorder::ResourceError,
        FormatError = QMediaRecorder::FormatError,
        OutOfSpaceError = QMediaRecorder::OutOfSpaceError
    };
    Q_ENUM(Error)

    explicit PuhuriAudioRecorder(QObject *parent = nullptr)
        : QObject(parent),
          m_recorder(new QAudioRecorder(this)),
          m_settings(m_recorder->audioSettings())
    {
        m_settings.setCodec(QStringLiteral("audio/x-opus"));
        m_settings.setEncodingMode(QMultimedia::ConstantBitRateEncoding);
        m_settings.setBitRate(32000);
        m_settings.setSampleRate(48000);
        m_settings.setChannelCount(1);
        m_recorder->setEncodingSettings(m_settings);
        m_recorder->setContainerFormat(QStringLiteral("audio/ogg"));
        m_recorder->setAudioInput(m_recorder->defaultAudioInput());

        connect(m_recorder, &QMediaRecorder::stateChanged, this,
                &PuhuriAudioRecorder::recorderStateChanged);
        connect(m_recorder, &QMediaRecorder::stateChanged, this,
                [this](QMediaRecorder::State state) {
                    if (state == QMediaRecorder::RecordingState)
                        m_hasRecorded = true;
                    finishIfReady();
                });
        connect(m_recorder, &QMediaRecorder::statusChanged, this,
                &PuhuriAudioRecorder::recorderStatusChanged);
        connect(m_recorder, &QMediaRecorder::statusChanged, this,
                [this](QMediaRecorder::Status) { finishIfReady(); });
        connect(m_recorder,
                QOverload<QMediaRecorder::Error>::of(&QMediaRecorder::error),
                this, [this](QMediaRecorder::Error) {
                    m_finishPending = false;
                    Q_EMIT errorChanged();
                });
        connect(m_recorder, &QMediaRecorder::durationChanged, this,
                &PuhuriAudioRecorder::durationChanged);
        connect(m_recorder, &QMediaRecorder::durationChanged, this,
                [this](qint64 duration) {
                    if (duration >= 120000)
                        m_recorder->stop();
                });
        connect(m_recorder, &QMediaRecorder::actualLocationChanged, this,
                &PuhuriAudioRecorder::actualLocationChanged);
    }

    State recorderState() const { return State(m_recorder->state()); }
    Status recorderStatus() const { return Status(m_recorder->status()); }
    Error errorCode() const { return Error(m_recorder->error()); }
    QString errorString() const { return m_recorder->errorString(); }
    QString outputLocation() const { return m_recorder->outputLocation().toString(); }
    QString actualLocation() const { return m_recorder->actualLocation().toString(); }
    qint64 duration() const { return m_recorder->duration(); }
    QStringList supportedAudioCodecs() const { return m_recorder->supportedAudioCodecs(); }
    QStringList supportedContainers() const { return m_recorder->supportedContainers(); }

    void setOutputLocation(const QString &location)
    {
        m_recorder->setOutputLocation(QUrl(location));
        Q_EMIT outputLocationChanged();
    }

public Q_SLOTS:
    void record() {
        m_hasRecorded = false;
        m_finishPending = true;
        m_recorder->record();
    }
    void stop() { m_recorder->stop(); }
    void pause() { m_recorder->pause(); }

Q_SIGNALS:
    void recorderStateChanged();
    void recorderStatusChanged();
    void errorChanged();
    void outputLocationChanged();
    void actualLocationChanged(const QUrl &);
    void durationChanged(qint64);
    void finished();

private:
    void finishIfReady() {
        // StoppedState can precede the muxer's final page. Wait for the
        // backend to leave FinalizingStatus before the file is staged.
        const auto status = m_recorder->status();
        if (m_finishPending && m_hasRecorded
                && m_recorder->state() == QMediaRecorder::StoppedState
                && (status == QMediaRecorder::LoadedStatus
                    || status == QMediaRecorder::UnloadedStatus)
                && m_recorder->error() == QMediaRecorder::NoError) {
            m_finishPending = false;
            Q_EMIT finished();
        }
    }

    bool m_finishPending = false;
    bool m_hasRecorded = false;
    QAudioRecorder *m_recorder;
    QAudioEncoderSettings m_settings;
};

class PuhuriAudioPlugin final : public QQmlExtensionPlugin
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID "org.qt-project.Qt.QQmlExtensionInterface")

public:
    void registerTypes(const char *uri) override
    {
        qmlRegisterType<PuhuriAudioRecorder>(uri, 1, 0, "AudioRecorder");
    }
};

#include "puhuri_audio_plugin.moc"
