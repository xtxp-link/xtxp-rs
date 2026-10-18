Feature: Late joiner

  @e2e:late_joiner_loads_in_one_update
  Scenario: A late joiner loads the full state in one update
    Given three peers have edited a drawing through the host
    When a fourth peer connects
    Then it receives the whole document in one update and its hash matches theirs
