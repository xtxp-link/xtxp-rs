Feature: Reconnect sync

  @e2e:three_peers_converge_through_chaos
  Scenario: Peers converge through a proxy that drops, delays and duplicates frames
    Given three peers editing through the chaos proxy, which drops, delays and duplicates frames
    When they all finish
    Then a late joiner sees the same document as an in-process replica of every peer's updates

  @e2e:offline_edits_survive_reconnects
  Scenario: A peer that drops and reconnects catches up
    Given three peers editing through the chaos proxy, which often drops connections
    When a peer's connection drops and it keeps editing offline, then reconnects with a fresh client ID
    Then all peers end with identical hashes

  @e2e:ysweet_restart_keeps_the_document
  Scenario: y-sweet restarts
    Given a document y-sweet has written to disk
    When y-sweet restarts
    Then a new peer loads the same document
