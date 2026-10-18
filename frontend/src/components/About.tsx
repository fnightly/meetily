import React, { useEffect, useState } from "react";
import { getVersion } from '@tauri-apps/api/app';
import Image from 'next/image';
import { useI18n } from '@/contexts/I18nContext';

export function About() {
    const { t } = useI18n();
    const [currentVersion, setCurrentVersion] = useState<string>('0.4.1');

    useEffect(() => {
        getVersion().then(setCurrentVersion).catch(console.error);
    }, []);

    const features = [
        { title: t('about.privacyTitle'), body: t('about.privacyBody') },
        { title: t('about.recordingTitle'), body: t('about.recordingBody') },
        { title: t('about.notesTitle'), body: t('about.notesBody') },
        { title: t('about.libraryTitle'), body: t('about.libraryBody') },
    ];

    return (
        <div className="p-4 space-y-5 h-[80vh] overflow-y-auto">
            <div className="text-center">
                <div className="mb-3">
                    <Image
                        src="icon_128x128.png"
                        alt={t('about.appName')}
                        width={64}
                        height={64}
                        className="mx-auto"
                    />
                </div>
                <h1 className="text-xl font-bold text-gray-900">{t('about.appName')}</h1>
                <span className="text-sm text-gray-500">v{currentVersion}</span>
                <p className="text-sm text-gray-600 mt-2 max-w-md mx-auto leading-relaxed">
                    {t('about.tagline')}
                </p>
            </div>

            <div className="space-y-3">
                <h2 className="text-base font-semibold text-gray-800">{t('about.featuresTitle')}</h2>
                <div className="grid grid-cols-2 gap-2">
                    {features.map((feature) => (
                        <div key={feature.title} className="bg-gray-50 rounded p-3">
                            <h3 className="font-bold text-sm text-gray-900 mb-1">{feature.title}</h3>
                            <p className="text-xs text-gray-600 leading-relaxed">{feature.body}</p>
                        </div>
                    ))}
                </div>
            </div>

            <div className="pt-2 border-t border-gray-200 text-center">
                <p className="text-xs text-gray-400">{t('about.footer')}</p>
            </div>
        </div>
    )
}
