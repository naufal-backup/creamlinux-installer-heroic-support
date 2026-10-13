import { useState, useEffect } from 'react'
import { EpicGame } from '@/types/EpicGame'
import { ActionButton, Button } from '@/components/buttons'
import { Icon } from '@/components/icons'

interface EpicGameItemProps {
  game: EpicGame
  installing?: boolean
  onInstall: (game: EpicGame) => void
  onUninstallScream: (game: EpicGame) => void
  onUninstallKoaloader: (game: EpicGame) => void
  onSettings: (game: EpicGame) => void
}

const EpicGameItem = ({
  game,
  installing,
  onInstall,
  onUninstallScream,
  onUninstallKoaloader,
  onSettings,
}: EpicGameItemProps) => {
  const [imageUrl, setImageUrl] = useState<string | null>(null)
  const [hasError, setHasError] = useState(false)

  useEffect(() => {
    if (game.box_art_url) {
      setImageUrl(game.box_art_url)
    }
  }, [game.box_art_url])

  const backgroundImage =
    imageUrl && !hasError
      ? `url(${imageUrl})`
      : 'linear-gradient(135deg, #232323, #1A1A1A)'

  const anyInstalled = game.scream_installed || game.koaloader_installed
  const isWorking = !!installing

  return (
    <div
      className="game-item-card"
      style={{
        backgroundImage,
        backgroundSize: 'cover',
        backgroundPosition: 'center',
      }}
    >
      {imageUrl && !hasError && (
        <img
          src={imageUrl}
          alt=""
          style={{ display: 'none' }}
          onError={() => setHasError(true)}
        />
      )}

      <div className="game-item-overlay">
        <div className="game-badges">
          <span className="status-badge epic">Epic</span>
          {game.source === 'legendary-standalone' ? (
            <span className="status-badge">Legendary</span>
          ) : (
            <span className="status-badge">Heroic</span>
          )}
          {game.scream_installed && <span className="status-badge smoke">ScreamAPI</span>}
          {game.koaloader_installed && <span className="status-badge smoke">Koaloader</span>}
        </div>

        <div className="game-title">
          <h3>{game.title}</h3>
        </div>

        <div className="game-actions">
          {/* Nothing installed - install button */}
          {!anyInstalled && (
            <ActionButton
              action="install_unlocker"
              isInstalled={false}
              isWorking={isWorking}
              onClick={() => { if (!isWorking) onInstall(game) }}
            />
          )}

          {/* ScreamAPI installed - uninstall + settings */}
          {game.scream_installed && (
            <ActionButton
              action="uninstall_smoke"
              isInstalled={true}
              isWorking={isWorking}
              onClick={() => { if (!isWorking) onUninstallScream(game) }}
            />
          )}

          {/* Koaloader installed - uninstall */}
          {game.koaloader_installed && (
            <ActionButton
              action="uninstall_smoke"
              isInstalled={true}
              isWorking={isWorking}
              onClick={() => { if (!isWorking) onUninstallKoaloader(game) }}
            />
          )}

          {/* Settings button - only for direct ScreamAPI (not Koaloader) */}
          {game.scream_installed && !game.koaloader_installed && (
            <Button
              variant="secondary"
              size="small"
              onClick={() => onSettings(game)}
              disabled={isWorking}
              title="Configure ScreamAPI"
              className="edit-button settings-icon-button"
              leftIcon={<Icon name="Settings" variant="solid" size="md" />}
              iconOnly
            />
          )}
        </div>
      </div>
    </div>
  )
}

export default EpicGameItem